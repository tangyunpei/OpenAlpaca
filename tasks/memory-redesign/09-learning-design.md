# Learning v2 — design and implementation plan

**Status: rev 3.33 (2026-09-27): implemented; the evaluation (L8) is running, not finished.** The owner answered LQ1–LQ4 and approved this design (§10); LQ5–LQ10, raised while building it, are pending (§10) and none is adopted; LQ11–LQ17, raised by the L8 evaluation, were taken on 2026-09-26 (§10), and the **L8 product slice** `6120812b..79f00f49` builds LQ11, LQ13, LQ14 and LQ16 (rev 3.33). Every slice is on `feat/markdown-memory`, unpushed, and each names the revision-log entries that record its deviations: **L1** `48f50793..f5cfc80b`, review fixes `21a8954f..f00bbc9a` (revs 3.1, 3.2); **L2** `33bf9226..fa9f4f1b`, review fixes `388ed329..cfb05abf`, one commit per finding (revs 3.3, 3.4); **L3** `9e533a66..c1794c42`, review fixes `5774e8cc..9b3dff27` and the re-review's `3f356e56` (revs 3.5, 3.6); **L4** `63b29ba9..31af54c4`, review fixes `d2f82075..d347c246` (rev 3.7); **L5** `5942c12f..e64151f2` (rev 3.8); **L6** `9ade5905..f35339de`, review fixes `57a7065d..77fdcf8f` (rev 3.9); **L7** `6b1f1d5d..aabbbd97`, review fixes `bfa78da3..fcc825f2` and the re-review's `2f685ca3` (revs 3.10–3.12); **L9** (§12, MD-13) `15318ddd..03c253d4` (rev 3.13); **L10a** (§13.1, MD-14) `b71d6e9d..95978e31`, review fixes `b7b87266`, `a90202ed` (rev 3.14); **L10b** (§13.2, MD-15) `19f46c61..9df70cf7`, review fixes `ac9d6bd9..3db79ce1` (revs 3.15, 3.16); **L10c** (§13.3, MD-16) `52668aca..fc5f5cce`, review fixes `5da63a2e..fa09dc0c` (revs 3.17–3.19); **L8-prep** (§13.4) `bee1bd27..50cf878d` (rev 3.20); the §7.1 second-session corpus review `7b240b27` (rev 3.21); and the **residue slice** R1–R8 `4f2eb5e5..da53e1a2` (rev 3.22; rev 3.23 corrects two lines of it). L8 — the runs and `10-learning-evaluation.md` — is running from a pinned worktree. Rev 2 applies a verified review of rev 1 (29 findings, all accepted, two of them in part; the revision log lists each). The design is written for the owner decisions MD-10, MD-11 and MD-12 and the adopted scope in [03](03-design.md) §0 (rev 5), and for MD-13…MD-16 (§12, §13); it was first written for the recommended answers to LQ1–LQ4, and rev 3 applied the owner's answers, which differ on LQ1.**

- **Source:** `feat/markdown-memory` at `55a1dc4c`. That is design rev 5 and plan rev 4.3, with slices 0–3 implemented. Every claim about current code cites `path:line` at that commit.
- **Where the facts come from:** three read-only surveys, of the learning pipeline, of the surfaces MD-11 and MD-12 touch, and of the six smaller items. They live in the session scratchpad and are not tracked. Everything this document relies on is restated here with its citation, so it stands without them.
- **Where the rulings are:** the rulings taken here are §9, and the changes to 03's frozen contracts are §11.

## 0. Scope

### 0.1 What the owner decided (03 §0, 2026-09-24)

> **MD-10** — "**Cadence:** every turn is queued; extraction runs over *all* turns since its last run when the conversation has been idle about 2 minutes, when 10 new turns have accumulated, when the user switches conversations, and before the daemon stops. Nothing is skipped, messages are no longer cut to 800/400 characters, and a reply is never delayed."
>
> **MD-11** — "**Replacement:** when learning finds that a new statement replaces an older note, the replacement is **automatic when the older note was itself learned** (shown in *recently learned* with one-click undo), and a **suggestion the user confirms when the older note was saved by the user** — G2 holds: nothing learned silently overrides a user-authored note."
>
> **MD-12** — "**Confirming a suggestion in chat:** the assistant may raise a suggested replacement, and the user answers on the same Approve/Deny card tool confirmations use; the Memory view and the CLI keep it too. The confirmation is the human's click, not the model's say-so."
>
> **Adopted scope:** "the extractor proposes **aliases (including the other language) and a `fact_key`**, reusing existing keys; every agent template gets memory **search** (`memory_read`); the recall header shows validity dates; a subagent's notes reach *recently used*; parsed partitions are cached; a 100K-note bench and the index size are examined; and a **learning evaluation** (conversations in, a different judge model) is added."

**The owner's principle:** users change memory through chat and rarely by hand. Automatic learning, and the models' ability to search memory, therefore matter most.

### 0.2 The diagnosis, checked at HEAD

| # | Finding | Evidence |
|---|---|---|
| D1 | Only every 5th turn per lane is extracted, and only that turn is. The counter lives in memory, restarts at 0 on every boot, and counts before the owner check. | `crates/openalpaca_core/src/orchestrator/extraction.rs:125-137`; `orchestrator/mod.rs:182`, `:380` |
| D2 | The user message is cut to 800 characters, the reply to 400 and the output to 512 tokens. The model sees no earlier turn, no existing note, no date. | `extraction.rs:185-187`, `:189-217`, `:233` |
| D3 | The prompt asks only for title, content, kind, stated_by and confidence. `StoreInput` sets no aliases, `fact_key`, validity or `supersedes`, and the service hard-codes `observed_at: None`. | `extraction.rs:196-203`, `:359-366`; `crates/openalpaca_core/src/memory/service/mod.rs:1032` |
| D4 | The extractor cannot read: `Capabilities::EXTRACTION` is write-only. | `memory/service/context.rs:67-72` |
| D5 | A supersede of a `user` note from a surface the user did not direct is refused at create time, whatever the new note's status. | `crates/openalpaca_storage/src/memory_files/partition.rs:982-984` |
| D6 | The evaluation switched learning off and graded with the reader model. | `crates/openalpaca_core/tests/memory_eval.rs:1680-1683`, `:1796`; [08](08-evaluation-report.md) §1 |
| D7 | `RecalledNote::render` prints no validity dates. | `memory/service/types.rs:385-417` |
| D8 | A subagent's distilled memory is recorded nowhere: `PackageSection` has no key, and `to_bundle` keys it `Package`. | `prompt_ctx/package.rs:58-65`, `:120`; plan §7a S3-R3 |
| D9 | Every operation re-opens and re-parses the partition: `store` parses 3 times, `update` 5–6 times, `forget` once per target. | `memory_files/partition.rs:736-799`, `:969`, `:1140`; `memory/service/mod.rs:1040-1042` |
| D10 | The 10K cache is ≈ 8× the corpus. 100K notes have never been run. | [08](08-evaluation-report.md) §4; `crates/openalpaca_storage/src/memory_index/bench.rs:27` |

The survey also found problems the owner did not list:
- **F7:** a follow-up replays the model-written `queue_followup` text as a `user` row with source `internal`, and the extractor treats that text as the user's own words (`apps/openalpacad/src/followup.rs:38-50`, `runner/lead_agent/tools.rs:1498-1515`).
- **The "daily" caps never reset and never bind at $0:** the cost tracker is seeded once at boot and nothing resets it (`crates/openalpaca_llm/src/routing/cost_tracker/mod.rs:238-241`, `apps/openalpacad/src/services/llm.rs:197-280`).
- **The extractor finds its source row by matching text** (`extraction.rs:448-463`), although the gateway already holds the row's id (`gateway/persistence.rs:26-34`).
- **Shutdown cancels learning and drains nothing** (`apps/openalpacad/src/main.rs:785-796`).
- **Undo of a learned replacement does not restore the note it replaced** (`memory_files/partition.rs:1158-1169`, `authority.rs:192-202`).

### 0.3 Non-goals

- **Group chat and connector audience.** MD-4 stays out. Connector lanes learn exactly as chat lanes do today, with no new audience rule.
- **A model-side confirm.** The model's say-so never mints origin `user`. `memory_update action: confirm` stays refused from a tool (G2). Only a human's Approve on a card for one exact call confirms (§4).
- **Embeddings, rerankers, query expansion or any model on the read path.** MD-1 stands: search stays lexical, and cross-language recall comes from the aliases the extractor now writes.
- **A daemon-raised confirmation card.** Option N in §4.1 is rejected.
- **Re-extracting historic sessions.** 03 §6 says "Historic sessions are not re-extracted by default". No backfill verb is built.
- **Changing when task-output extraction runs.** It stays per finished lead run (`orchestrator/dispatcher/memory.rs:21-64`). It gains only the shared item fields (§2.7) and the durable daily gate (§1.8).
- **Fixing the summary job's cost cap or its lack of a cancellation token** (`orchestrator/summary.rs:14-240`). Both are recorded as residue.
- **A verb that restores a note a confirmed replacement retired.** 03 §6 says "restoring an old note is an explicit update that removes the link", and no verb does that at HEAD: `NotePatch` has no `supersedes` field (`memory_files/note.rs:1143-1152`) and the CLI editor refuses one (`apps/openalpaca/src/commands/memory.rs:925-929`). MD-11 does not need it: undo (§3.3) brings back what an *automatic* replacement retired, and a confirmed suggestion is the user's own act. The gap stays as it is and is recorded as residue.
- **Cross-partition same-key precedence in recall.** 03 §3 rule 3 lets a project note outrank a global one for the same `fact_key`, but `same_key_loser` is derived per partition (`memory_files/authority.rs:215-225`) and recall filters only that flag (`memory_index/search.rs:223`). Learning avoids creating such pairs (§2.5); fixing recall is not this wave.

---

## 1. Cadence (MD-10)

### 1.1 The unit of learning is a session

Extraction state is kept **per session**, not per lane:
- Message rows are indexed by `(session_id, id)` (`crates/openalpaca_storage/src/migrations/001_baseline.sql:241`).
- R48 gives a session exactly one project (`gateway/persistence.rs:71-104`).
- "The user switches conversations" means the session changes.
- A lane spans sessions and projects. `{owner}:gui` is shared by the GUI and the CLI (`apps/openalpacad/src/main.rs:239`, T18).

Two things follow for each window:
- **Each row learns into its own turn's project (ruling LR1).** 03 §7a's Extraction row takes the project from "the turn's `request_workspace_root`", and 03 §3 rule 1 says a request with no workspace "reads and writes only the owner's global partition". The session's binding cannot stand in for that: an unbound session is bound by the first turn that names a workspace (`repository/conversation/mod.rs:364-378`), and a turn with no workspace never switches sessions (`gateway/persistence.rs:71-104`). So five project-less GUI turns followed by one that names project P share P's session, and so do GUI turns after a CLI turn switched the shared `{owner}:gui` session (T18). Migration 044 therefore stores each user row's root in a new `conversation_messages.workspace_root` column (§1.3): the canonical root the gateway already resolves for the turn (`gateway/router/mod.rs:317-325`), NULL when the turn named none.
  - A row with a root learns into that project plus the home partition. A row with none learns into the home partition only.
  - A **chunk never mixes roots** (§1.6): it ends where the root changes. An assistant reply learns under its user row's root.
  - R48 opens a new session whenever a turn names a different project, so within one session the roots are only ever NULL or that one project.
- **Its owner comes from the lane.** The owner is the user part of `session.lane_key`. A linked connector user's lane carries their `global_id` (`gateway/router/mod.rs:269-272`). The service's own check then applies unchanged (`memory/service/mod.rs:275-293`): an owner that is not a `global_user` row learns nothing, and its cursor advances.

### 1.2 The window: which rows, and what text

A **window** is the session's rows with `id > cursor`, in id order. It is read by a new `ConversationRepository::learning_window(session_id, after_id, limit)`. That query uses the existing `idx_conv_msg_session` index and selects only `id, role, content, source, created_at, request_id, workspace_root`. **It never reads `content_json` or `display_text`**, because those carry an attachment's extracted text (`gateway/persistence.rs:153-189`). This is how U2 and plan 3.4 hold for a database window: an uploaded document still cannot mint an `extracted_user` note.

Each row is classified. The classification is exact because of the new `request_id` column (§1.3):

| Row | In the window as | Why |
|---|---|---|
| `role='user'`, `request_id` set, `source <> 'internal'`, content not starting with `/`, at most 8 KiB | **user-authored** | a turn the user typed |
| the same, over 8 KiB | **user (long; may quote other text)** | a pipe sends all of stdin as one user message (`apps/openalpaca/src/commands/chat.rs:370-395`) and a paste is indistinguishable from typing. An item that cites it is never `UserWords` (§2.4 rule 5), so a piped email or web page yields candidates, not active notes that could replace learned ones automatically (ruling LR21) |
| `role='user'`, `source='internal'` | **assistant-written task** ("queued task (written by the assistant)") | F7: the model's own `queue_followup` text (`followup.rs:38-50`); an item that cites it is never `UserWords` (§2.4 rule 5) |
| `role='user'` whose content starts with `/` | left out, together with its answer | today's filter (`extraction.rs:119-123`) covers slash skills, `/steer` and scheduled skills (`scheduled_skills.rs:190-194`) |
| `role='assistant'` whose `request_id` matches an included user row | **assistant reply** | the turn's answer |
| `role='assistant'` with `request_id` NULL | left out | extension notices (`apps/openalpacad/src/notification/mod.rs:294-306`), `post_update` notes (`runner/lead_agent/tools.rs:1306-1319`) and completion reports (`dispatcher/outcome.rs:631-680`) are not turns; task extraction covers run output (ruling LR2) |
| a user row whose `request_id` is in flight (§1.5) | the window **stops before it** | its answer has not landed |

Up to two rows **before** the cursor (the last included user row and its reply, each cut to 1 KiB) are shown as `context`. A context row is removed from an item's citations and never becomes a source; an item left citing no other row is dropped (§2.4 rule 4). Rows with `session_id` NULL are never learned. The gateway always resolves a session before it writes (`gateway/persistence.rs:116-140`), so only writers outside the gateway produce such rows.

**The residual next to U2.** A shorter paste or a quoted passage inside a typed message is still a user-authored row. The prompt tells the extractor that a user row may quote other text and to extract only what the user says about themselves or their work (§2.1 rule 2); nothing deterministic proves it.

### 1.3 Durable state: migration 044

The schema baseline is 43, and the next migration is 44 (`crates/openalpaca_storage/src/migrations/mod.rs:15-20`). Migration **044**, file `044_learning.sql`:

```sql
-- 044: Learning v2 (tasks/memory-redesign/09): the turn id and project root on each message and a per-session learning cursor.
ALTER TABLE conversation_messages ADD COLUMN request_id TEXT;
ALTER TABLE conversation_messages ADD COLUMN workspace_root TEXT;

CREATE TABLE learning_cursor (
    session_id TEXT PRIMARY KEY,
    extracted_through INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Historic sessions are not re-extracted (design §6): every existing session starts at its newest message.
INSERT INTO learning_cursor (session_id, extracted_through)
    SELECT s.id, COALESCE((SELECT MAX(m.id) FROM conversation_messages m WHERE m.session_id = s.id), 0)
    FROM session s;

INSERT INTO schema_version(version) VALUES (44);
```

- **`request_id`.** Both gateway halves write it with the turn's request id. `persist_user_message[_with_attachments]` and `persist_assistant_message` (`gateway/persistence.rs:116-306`) gain the parameter, and `insert_row` writes it (`repository/conversation/mod.rs:101-129`).
  - Every other writer leaves it NULL, which is what marks a non-turn row.
  - `ConversationMessage` derives `Default`, so the new field costs one line per query (`crates/openalpaca_storage/src/models/conversation.rs:5-12`).
  - The field is `#[serde(skip)]`. `GET /v1/chat/history` serializes this struct directly (`apps/openalpacad/src/routes/chat_types.rs:94`), and the column stays off the wire.
- **`workspace_root`.** Only `persist_user_message[_with_attachments]` writes it, with the `workspace_path` it already receives: the canonical marker-walk root the gateway resolves for the turn (`gateway/router/mod.rs:317-325`), NULL when the turn named no project (§1.1). It is `#[serde(skip)]` like `request_id`. Assistant rows leave it NULL; they learn under their user row's root.
- **`learning_cursor` is its own table, not a column on `session` (ruling LR4).**
  - It leaves the session projection alone: `SESSION_COLUMNS`, `row_to_session` and the session wire are unchanged.
  - It keeps `attempts` and `last_error` across restarts, for the bounded retry in §1.4. The retry backoff is in memory only; the boot catch-up restarts it.
  - It is named without the `memory_` prefix, because `a_fresh_database_opens_at_43_with_no_memory_tables` fails on any table matching `memory!_%` (`crates/openalpaca_storage/src/database/tests.rs:369-380`).
  - A session with no row reads as `extracted_through = 0`. Every session created after 044 holds only post-044 messages, so that is correct.
- **Lifecycle.**
  - `delete_session` deletes the cursor row in its existing transaction (`repository/conversation/mod.rs:537-570`).
  - `DELETE /v1/chat/history` needs nothing. Message ids are `AUTOINCREMENT` and never reused (`001_baseline.sql:222`), so a cursor above the cleared rows still reads correctly.
  - A factory reset deletes the database, and the cursor with it.
  - An orphaned row, whose session is gone, is harmless: the catch-up query in §1.5 joins from messages.
- **Rejected places for the watermark.**
  - `system_config`: `openalpaca config reset` wipes it (`repository/config/mod.rs:91-100`).
  - The control snapshot: its schema is frozen with `deny_unknown_fields` (`memory_files/snapshot.rs:124`). It is per partition, while one window writes to two. `doctor --repair` rebuilds it, and a factory reset keeps it while the database goes.
  - In-memory state alone: it is lost on restart, and MD-10's "before the daemon stops" cannot be met inside the 10 s exit window (`main.rs:71`).

### 1.4 Crash semantics: at-least-once, with writes a repeat cannot harm

SQLite and the note files cannot share a transaction, so a window cannot be exactly once. The choice is **at-least-once** (ruling LR5):

1. Read the window's chunk (§1.6).
2. Call the model.
3. Write the notes. Each is one crash-safe single-file mutation (03 §6).
4. Advance the cursor to the chunk's last row id by compare-and-swap:

   ```sql
   INSERT INTO learning_cursor(session_id, extracted_through, attempts, last_error, updated_at)
   SELECT ?1, ?2, 0, NULL, datetime('now') WHERE EXISTS (SELECT 1 FROM session WHERE id = ?1)
   ON CONFLICT(session_id) DO UPDATE SET extracted_through = excluded.extracted_through, attempts = 0,
       last_error = NULL, updated_at = excluded.updated_at
   WHERE learning_cursor.extracted_through = ?3
   ```

A crash between steps 3 and 4 re-runs the same chunk. At-most-once (advance first) would silently lose the window, which contradicts "nothing is skipped".

**Steps 3 and 4 are one blocking task.** The chunk's store loop and its cursor advance run together in one `spawn_blocking` closure (§1.7). A cancellation or a shutdown that lands after the model answered therefore never separates them. The flush's async half may be dropped, but the closure finishes, and only a process exit mid-closure can leave the cursor behind the writes.

Four guards make a re-run harmless. Each is deterministic and needs no model:

- **(a) Exact dedup.** A byte-identical item answers `dedup` with the existing id (03 §7b P8, Q8; `partition.rs:985-1001`). That covers effectively active notes only; S4 (§3.2) extends it to identical pending suggestions.
- **(b) Already learned (§2.4 rule 13).** The known-notes lookup (§2.3) always collects the notes, **active or candidate**, whose `sources` cite a row of this chunk and whose origin is `extracted_user` or `extracted_other`, as `from_this_window`. No surface is recorded per note (the snapshot keeps `{revision, origin, written_at}`, `memory_files/snapshot.rs:124`), so a `ModelTool` note citing the same row counts too; that is correct, since the model's own `memory_store` already learned it. An item is dropped as `already_learned` when such a note is a candidate naming the same target the item would (a pending suggestion or inferred replacement of T), or when it has the item's identity (equal `fact_key` when both carry one, else equal `kind` and case-folded title) **and** its `## Knowledge` section equals the item's content after normalisation (§2.5). A same-key item whose value differs is not dropped: guard (c) lets it correct the earlier copy.
- **(c) Same key is a replacement.** A re-extracted keyed item that slipped past (b) supersedes its own earlier *active* copy (§2.5, ruling LR11). Only one note stays recallable.
- **(d) No resurrection.** Before storing, every cited message is checked with `ControlSnapshot::is_source_suppressed("message:<id>")` on both partitions. Today this function has no production caller (`memory_files/snapshot.rs:305-310`). A suppressed row is removed from the item's citations, and an item left with none is dropped. This makes 03 §6's hard guarantee ("no retrieval or replay resurrects the known records or source ids") hold for replays. Without it, a re-extracted note written after the forget is *not* derived `source_forgotten`, because that rule requires `written_at <= forgotten_at` (`memory_files/authority.rs:245-253`). Only an explicit **forget** suppresses sources; a withdrawal (undo, a dismissal, a Deny) does not (§3.3, ruling LR12).

Three residuals are accepted and stated. Each needs a crash between steps 3 and 4, which is now only a process exit mid-closure:
- a *reworded, keyless* re-extraction can leave a near-duplicate learned note, which the user sees in *recently learned* and can undo;
- a reworded re-extraction of an inferred candidate that has no target can leave a second candidate, outside recall;
- a suggestion dismissed after the crash and before the boot catch-up re-runs the chunk can be raised once more.

**Bounded retry (ruling LR20).** Only a **reachable model's bad output** counts: output whose top level is not `{"items": [...]}`, or an `EmptyCompletion`. It leaves the cursor where it is and raises `attempts` with `last_error`. After `MAX_ATTEMPTS` (3) consecutive such failures of the same chunk, the cursor advances past it with one WARN naming the session, the row range and the error. One poisoned chunk therefore cannot block a session forever.

Everything else **holds** the cursor, counts nothing, and retries the session with an in-memory backoff (60 s, doubling, capped at 1 h; the sweeper skips a session until its backoff has passed):
- **the model is unavailable**: any router error, including `NoRoutableModel`, a network failure, an Ollama that is not running, or exhausted rate-limit retries. Today this is 03 §7b P10's `unavailable` (`orchestrator/extraction.rs:258-264`). It is logged with **one WARN when an outage starts** and one INFO when learning resumes, never once per retry, so fixture 19's "one visible line … no retry loop" still reads true: nothing loops, and the turn is untouched;
- **a partition is quarantined** (`refuse_if_quarantined`, `memory_files/partition.rs:957`; 03 §2). The check runs in `learning_view`, *before* the model call, so no cost is spent on a window that cannot be written. The window waits for `memory doctor --repair`; 03 §2 quarantines "extraction … for that partition" and this is what that means for a queue;
- **a store I/O error** during the store loop. The notes already written answer `dedup` or `already_learned` on the retry.

A cap binding (§1.8), a yield to chat (§1.7) or a cancellation also leaves the cursor unchanged and counts nothing.

### 1.5 The queue and its four triggers

`LearningQueue` is a new type in `crates/openalpaca_core/src/memory/learning/queue.rs`. It lives in core, because the evaluation harness builds no daemon (§7.2). It owns its own `BackgroundJobs`, the same type as the orchestrator's (`orchestrator/background_jobs.rs:17-72`), so every flush is tracked and cancellable. In memory it keeps three things:
- per session: `pending_user_turns`, `last_turn_at`, `due`, the retry backoff (§1.4), and whether a flush is running or queued to run again;
- per lane: the current session;
- the set of in-flight request ids.

It has two modes. `Automatic` is the daemon's: the sweeper and the triggers below run. `Manual` is the evaluation harness's: no sweeper and no trigger starts a flush, and only `flush_session`, `flush_all` and `wait_idle` do (§7.2).

**How the gateway feeds it.** The gateway gains `with_learning(Arc<LearningQueue>)`, and the daemon sets it on its one production gateway (`apps/openalpacad/src/main.rs:518`). `handle_event` creates an RAII turn guard **before it persists the user row** (`gateway/router/mod.rs:293-325`), right after the request id is minted:
- `turn_started(request_id, lane_key)` inserts the request id into the in-flight set. The id is in flight before its row exists, so a flush that reads the window between the insert and the reply stops before the row (§1.2) instead of treating it as a finished turn and moving the cursor past its answer.
- Once persistence resolves the session, `attach_session(request_id, session_id)` records it.
- The guard's drop runs `turn_finished(...)` after the assistant row is written (`:414-435`), or on any earlier exit. It removes the id, stamps `last_turn_at`, and counts the turn when it was user-authored.
- Each is one `std::sync::Mutex` update. There is no await and no database access.

**The triggers.** A flush starts only when no chat turn is in flight on the daemon (§1.7).

| Trigger | Rule | Source |
|---|---|---|
| **Idle** | A session with `pending_user_turns > 0` whose `last_turn_at` is at least `learning.idle_secs` (120) old becomes due. A sweeper in the queue checks every 15 s, so a session becomes due between 120 and 135 s after its last turn. `idle_secs = 0` switches this trigger off. | in memory; at boot, `session.last_message_at` (`repository/conversation/mod.rs:754-765`) |
| **10 turns** | The `learning.turns_per_flush`-th (10) user-authored turn since the cursor makes the session due at once. | `turn_finished` |
| **Switch** | The same sweeper, on each tick, reads `session.status` for the sessions it holds with pending turns (one `SELECT id FROM session WHERE id IN (…) AND status = 'archived'`, on `spawn_blocking`). A session that is no longer its lane's active one is due. Every switch archives the old session: "New chat" and the R48 project switch through `create_session`, and reopening an old conversation through `activate_session` (`repository/conversation/mod.rs:408-424`, `:439-455`). No bus subscriber is needed. | the sweeper |
| **Stop** | `LearningQueue::drain(budget)` marks every session with pending turns due and starts flushes, ignoring the quiet rule because the server has already stopped. It waits up to `budget`, where `budget = min(SHUTDOWN_DRAIN (3 s), what is left of FORCE_EXIT_GRACE − 5 s)`. `main.rs` calls it after `server.await` returns (`:781`) and before the post-turn jobs stop. The queue's own `shutdown(3 s)` then runs concurrently with `orchestrator.background().shutdown(3 s)` (`:785-796`), so the cost flush, the session-log flush and the extension sweep keep at least 4 s of the 10 s window. | `main.rs` |
| **Boot catch-up** (the durable half of Stop) | `BOOT_CATCHUP_DELAY` (30 s) after start, one query finds every session with unlearned user-authored rows. Each such session is marked due, and the sessions are flushed one at a time when the daemon is quiet. The query: `SELECT m.session_id FROM conversation_messages m LEFT JOIN learning_cursor c ON c.session_id = m.session_id WHERE m.role = 'user' AND m.request_id IS NOT NULL AND m.source <> 'internal' AND m.id > COALESCE(c.extracted_through, 0) GROUP BY m.session_id` | `main.rs`, after the orchestrator is built |

**Stop, stated honestly.** On a local model, one extraction call takes tens of seconds ([08](08-evaluation-report.md) §3 reader p50 is 11.5 s). The drain therefore rarely finishes there, and a cloud call sometimes does. What MD-10's "before the daemon stops" guarantees is that **nothing is lost**: what the drain does not finish, the boot catch-up learns.

**Deleting a conversation.** Following the recommended answer to LQ4:
- `DELETE /v1/sessions/{id}`, `DELETE /v1/chat/history` and the workspace purge drop the session's queue state, and its unlearned turns go with the messages.
- A flush already in flight re-checks that the session exists after the model call and before writing, and writes nothing if it is gone.
- A deletion that races the writes is caught by `verify_sources`, which refuses a message id that no longer exists (`memory/service/mod.rs:860-903`).

### 1.6 Chunking and the byte budget

"Messages are no longer cut to 800/400" (MD-10). The replacement is a **byte budget that scales with the extraction model's context window**. It is computed per flush from `orchestrator::attachment_adapt::answering_model(router, extraction_model)` (`orchestrator/attachment_adapt.rs:53-68`), which is the one resolution M5, S3 and U1 already use:

```text
window  = AnsweringModel.window, or 8192 tokens when unknown (Ollama's discovery fallback)
out_tok = min(2800, window / 4)                                   max_tokens of the request
items   = min(costs.extract_max_items_per_call (12), (out_tok − 300) / 180)
in_bytes = (window − out_tok − 1000) × 2                          2 bytes per token: conservative for CJK
transcript_bytes = min(MAX_TRANSCRIPT_BYTES (24 KiB), in_bytes × 7 / 10)
known_bytes      = min(8 KiB, in_bytes × 3 / 10)
```

- **What the budget gives.** An 8,192-token local model gets 7,201 bytes of transcript, 3,086 bytes of known notes and 9 items. A 128K model gets 24 KiB, 8 KiB and 12 items.
- **A keyed provider's rate budget caps it too.** On a provider that needs a key, `in_bytes` is also capped so that the chunk's estimated input (the router's own bytes ÷ 4, `routing/router/capacity.rs:255-281`) is at most 40 % of that key's TPM capacity, read through `rate_headroom` (§1.7). At the default 40,000 TPM (`routing/rate_limiter/mod.rs:415-417`, `default_tpm`) the cap is 16,000 tokens less the fixed request overhead: `in_bytes ≤ TPM × 1.6 − REQUEST_OVERHEAD_BYTES` (6 KiB since L2, rev 3.3), 56.5 KiB, and it never binds there; it binds only on a key configured far below the default, and below about 3,000 TPM a keyed session never learns — the fixed prompt, its context and one cut turn alone pass half the bucket, so the session stays due (`memory/learning/mod.rs`, `Budget` and `REQUEST_OVERHEAD_BYTES`; revs 3.2, 3.3).
- **Cuts to single rows.** These apply only to single rows, and never at 800/400:
  - a user-authored row over 8 KiB keeps its first 6 KiB and last 2 KiB, around a marked `[… N bytes omitted …]`;
  - an assistant reply or assistant-written task over 4 KiB keeps 3 KiB + 1 KiB the same way;
  - a context row keeps 1 KiB.
- **Chunks.** A window larger than `transcript_bytes` is split at row boundaries into consecutive chunks. A chunk also ends where the rows' `workspace_root` changes (§1.1). A user row and its reply stay together, and one turn always fits, because the per-row cuts come first.
  - Chunks run in order. Each has its own known-notes lookup, so chunk 2 sees chunk 1's notes.
  - Each chunk advances the cursor, so a crash repeats at most one chunk.
  - A flush runs at most `MAX_CHUNKS_PER_FLUSH` (4) chunks. The rest stays due and runs at the next start that is quiet.
- **The minimum.** A chunk whose user-authored text totals less than `costs.extract_min_content_len` (20 bytes) is not sent, and the cursor advances past it. This is today's filter (`extraction.rs:119-123`), applied to the chunk rather than the turn.

### 1.7 Concurrency, yielding to chat, cancellation

- **One flush per session.** A trigger that fires while that session's flush runs sets `rerun`, and the queue re-checks after the flush ends.
- **One flush per daemon** (`MAX_CONCURRENT_FLUSHES = 1`). A local model serves one request at a time, and learning is background work.
- **Start only when quiet, chunk by chunk.** A due session starts when the in-flight set is empty and has stayed empty for 20 s (`RESUME_QUIET_SECS`). The same check runs again before **every** chunk, not only before the first, so a turn that starts mid-flush stops the flush at the next chunk boundary; the session stays due.

A flush shares the model with chat in one of two ways, and each gets its own rule (ruling LR7):

- **A local model: yield.** The extraction model's provider is *local* when it needs no key (`LlmRouter::provider_requires_key`, `routing/router/mod.rs:415`; false only for Ollama, `crates/openalpaca_llm/src/lib.rs:75`, `providers/ollama.rs:184`), **or** when its configured `base_url` (`LlmRuntimeConfig.provider_defaults[..].base_url`, `config/llm_config/runtime.rs:105-121`) names a loopback host (`localhost`, `127.0.0.0/8`, `::1`). The second clause covers llama.cpp or LM Studio behind the `openai` provider, which needs a key and would otherwise never yield. On a local model a `turn_started` cancels the flush's model call through a per-flush child `CancellationToken`. Nothing is written, no attempt is counted, and the session stays due. The GPU then serves the reply alone.
- **A keyed remote model: leave half the rate budget to chat.** A remote call cannot be cancelled without paying for it, and it shares the key's buckets with every chat round: each router call waits in `key_limiter.acquire(estimated_tokens)` until the RPM and TPM buckets hold enough (`routing/router/retry.rs:36`, `:64`; `routing/rate_limiter/mod.rs:327-350`). Four back-to-back 24 KiB chunks would take most of a default 40,000-token bucket, and the next chat round would sleep for the deficit. So `openalpaca_llm` gains one read-only method, `LlmRouter::rate_headroom(model) -> Option<RateHeadroom { rpm_available, rpm_capacity, tpm_available, tpm_capacity }>`. It returns the minimum over the model's provider's key limiters, read with the existing non-consuming `TokenBucket::available()` (`rate_limiter/mod.rs:101-105`). A chunk starts only when, **after** its own estimate, at least half of each bucket would remain. Otherwise the session stays due, counts nothing, and is looked at again on the next sweep. With the §1.6 cap, a chunk can always start once the key is idle. A chat round that begins while a chunk is in flight therefore finds at least half of the key's RPM and TPM. It can still wait only when its own demand exceeds that half, which chat alone can cause too.
- **Cancellation.** The queue's shutdown token and the per-flush yield token are selected against the model call, as today (`extraction.rs:251-266`), and checked between chunks. A chunk's store loop **and its cursor advance** run as one `spawn_blocking` closure (§1.4): at most 12 single-file writes and one `UPDATE`, with no model. Today `store` blocks a tokio worker on a `Condvar` (`memory/service/write_slot.rs:1-60`, `extraction.rs:367`). This removes that.

### 1.8 Cost bounds

| Bound | Value | Where |
|---|---|---|
| Per call | `transcript_bytes` + `known_bytes` of input (§1.6), `out_tok` ≤ 2,800 output tokens, ≤ 12 items, `thinking: Disabled` (M2, as today at `extraction.rs:240`) | `memory/learning/extractor.rs` |
| Per flush | ≤ `MAX_CHUNKS_PER_FLUSH` (4) calls, each started only with half the key's rate budget left over (§1.7) | queue |
| Per UTC day, cost | `costs.extract_max_daily_cost_usd` (0.25, unchanged default) | durable, below |
| Per UTC day, calls | `costs.extract_max_calls_per_day` (**new**, default 200, `0` = no cap). This is what bounds a $0 local model, where the cost cap never binds. | durable, below |

**The daily-cap defect, fixed for both extractors (ruling LR8).**
- Today the gate reads the in-memory `CostTracker`. It is seeded at boot and never reset (`cost_tracker/mod.rs:238-241`, `services/llm.rs:197-280`), so "daily" means "since boot".
- The two extraction gates now read the durable `llm_usage_daily` for today's **UTC** date and `agent_id = "orchestrator_user_extract"` (or `"orchestrator_task_extract"`). They use a new `LlmUsageRepository::usage_today(agent_id) -> (requests, cost_usd)`, which sums over models, beside `get_today_usage` (`repository/llm_usage/mod.rs:346-368`). `record_and_log` already upserts that row on every call (`:374-419`), including an extraction whose JSON failed (`extraction.rs:284-301`).
- When a cap binds, the window **stays queued**, with one INFO per session per day. Learning lags and loses nothing. It resumes after 00:00 UTC.
- The summary job's cap still reads the tracker. That is recorded as residue.

The 0.25 USD default is kept until L8 measures cost per flush on a cloud model (§7.3). If typical daily spend exceeds it, the owner is asked then, with numbers.

### 1.9 What a reply never waits for

- **The reply path does no learning work.** Its only learning work is the gateway's three in-memory updates (§1.5). It never reads the cursor, never counts rows and never takes the memory write slot.
- **Learning's own work stays off the tokio workers.**
  - Window reads, `learning_view` (§2.3) and cursor writes run on `spawn_blocking`.
  - Note writes run on `spawn_blocking`, and each takes its partition's write slot for one note.
  - A concurrent recall search can wait on the index connection for one note's reindex, which takes milliseconds.
- **Model contention is handled by §1.7.** A flush starts only after 20 s of quiet and re-checks before each chunk. On a local model it yields the moment a turn starts. On a keyed remote model it starts a chunk only when half the key's rate budget would remain.
- **The one per-turn read learning adds** is L4's `<memory_suggestions>` block. It reads the index, not the partition (§4.5), and re-reads at most ten files.
- **The post-turn job loses its extraction.** In `handlers.rs:331-396` it keeps only the summary, and `extraction_turn_counter` is deleted (`orchestrator/mod.rs:182`, `:380`).

### 1.10 Configuration

| Key | Fate | Default, clamp |
|---|---|---|
| `[orchestrator.learning] enabled` | **new**; the off switch, which the evaluation uses | `true` |
| `[orchestrator.learning] idle_secs` | **new**; MD-10's "about 2 minutes" | 120, 0..=3600 (0 = off) |
| `[orchestrator.learning] turns_per_flush` | **new**; MD-10's "10 new turns"; replaces `extract_every_n_turns` | 10, 1..=100 |
| `[orchestrator.costs] extract_every_n_turns` | **removed** (no compatibility code; the app is unreleased) | — |
| `[orchestrator.costs] extract_max_items_per_turn` | **renamed** `extract_max_items_per_call` | 12, 1..=50 |
| `[orchestrator.costs] extract_max_calls_per_day` | **new** | 200, 0..=10000 (0 = no cap) |
| `[orchestrator.costs] extract_max_daily_cost_usd`, `task_extract_max_daily_cost_usd` | kept; now read durably (§1.8) | 0.25, 0.50 |
| `[orchestrator.costs] extract_min_content_len` | kept; now per chunk | 20 |
| `[orchestrator.costs] extraction_model` | kept | router default |
| `[orchestrator.memory] memory_confidence_threshold` | kept; **gains** its registry row, its CLI row and a `config/daemon.toml` line, all of which it lacks today | 0.65, 0..=1 |

**Constants, not keys.** These are engineering bounds, not owner-facing choices, and live in `memory/learning/mod.rs`: `MAX_TRANSCRIPT_BYTES` 24 KiB, `MAX_CHUNKS_PER_FLUSH` 4, `MAX_ATTEMPTS` 3, `SHUTDOWN_DRAIN` 3 s, `BOOT_CATCHUP_DELAY` 30 s, `RESUME_QUIET_SECS` 20 s, and the backoff (60 s doubling to 1 h). Only MD-10's two numbers, the off switch and the new daily call cap are keys.

Rows change in four places:
- `crates/openalpaca_core/src/daemon_config/orchestrator.rs:216-275`
- `daemon_config/validation.rs:44-86`
- `crates/openalpaca_storage/src/config_schema/registry.rs:308`, `:372`, `:385`
- `apps/openalpaca/src/commands/daemon_config_cli/mod.rs:98-127`

**Registry rows.** The three `[orchestrator.learning]` keys, `extract_max_calls_per_day` and `memory_confidence_threshold` each get a registry row and a CLI mapping. `extract_every_n_turns` loses its row, and the renamed key keeps one. The Daemon category therefore goes from 43 keys to **47** (`config_schema/tests.rs:101`).

The only `daemon.toml` file to change is `config/daemon.toml:39-51`. The release template has no `[orchestrator.costs]` table (`scripts/release/templates/config/daemon.toml`).

---

## 2. Richer extraction (MD-11's input, the adopted aliases and `fact_key`)

`orchestrator/extraction.rs` is replaced by `crates/openalpaca_core/src/memory/learning/extractor.rs`, which sits beside `memory/task_extraction/`. It keeps `AGENT_LABEL = "orchestrator_user_extract"` (`extraction.rs:50`). The request keeps `temperature 0.0`, `thinking: Disabled`, no caching and no fallbacks, and records usage through `record_and_log` exactly as now (`extraction.rs:222-316`).

### 2.1 The prompt contract

**The verbatim text is a fixture.** L2 commits the system text and the user-message template as `memory/learning/extractor_prompt.txt` (read with `include_str!`), and a golden test renders one fixed chunk and compares it byte for byte (`the_extractor_prompt_matches_its_fixture`). What follows is that text's contract; two implementers cannot diverge from a committed file.

**System:** `<role>You are the memory extractor for OpenAlpaca.</role>`, plus today's JSON-only line (`extraction.rs:225-228`).

**User message, in this order:**

```text
<today>2026-09-24</today>                         UTC date of the chunk's last row
<known_notes>                                     §2.3; empty element when none
n1 key=user.editor origin=learned kind=preference scope=global "Editor preference" — The user edits with vim.
n2 key=user.laptop origin=saved-by-you kind=fact scope=global "Work laptop" — …
n3 key=- origin=learned kind=episode scope=project "Lantern outage" — … [from this conversation]
</known_notes>
<known_keys>user.editor, user.laptop, user.commute, lantern.staging.db, …</known_keys>
<conversation>
R1 context user 2026-09-24T10:00:03Z: …
R2 context assistant: …
R3 user 2026-09-24T10:02:11Z: …
R4 assistant 2026-09-24T10:02:40Z: …
R5 queued-task (written by the assistant) 2026-09-24T10:05:00Z: …
R6 user (long; may quote other text) 2026-09-24T10:07:30Z: …
</conversation>
<output_schema>…below…</output_schema>
<extraction_rules>…below…</extraction_rules>
```

**The `<output_schema>` element, verbatim:**

```text
{"items": [{
  "title": "a short title, at most 80 characters",
  "content": "the knowledge in one to three sentences, in the user's language",
  "kind": "preference" | "fact" | "decision" | "procedure" | "reference" | "episode",
  "stated_by": "user" | "assistant",
  "confidence": 0.0 to 1.0,
  "rows": [the numbers of the rows that state it, e.g. 3],
  "aliases": ["other names a later question might use"],
  "fact_key": "subject.attribute" | null,
  "replaces": "n1" | null,
  "replace_reason": "why it replaces that note" | null,
  "valid_from": "YYYY-MM-DD" | null,
  "valid_to": "YYYY-MM-DD" | null
}]}
```

**Extraction rules.** The prompt states them in these words or tighter:
1. Extract durable knowledge **about the user** that is clearly stated or strongly implied: preferences, facts about them or their work, decisions, procedures, references and episodes. Do not invent anything.
2. Use `stated_by: "user"` only when a `user` row says it in the user's own voice, and then cite **only** `user` rows. A user row may quote other text, such as an email, a web page or a log; extract from a quotation only what the user says about it. Anything supported by `assistant`, `queued-task` or long `user` rows is `"assistant"`.
3. Cite in `rows` the numbers of the rows that state it. A `context` row is never enough on its own.
4. When the conversation shows a value changing, output **only the current value**, say in `content` what it replaced and when, and set `replaces` if that older value is one of the `known_notes`.
5. Give `aliases`: other names a later question might use, **including the other language** (Chinese for English statements and English for Chinese ones), plus common alternative spellings and proper nouns. Give 0–8. Never repeat the title.
6. Give `fact_key` as `<subject>.<attribute>[.<qualifier>]`, in lowercase English. The subject is `user` for anything about the user themselves: their devices, commute, habits, people, schedule. It is the project's name for a fact about a project. **Reuse a key from `known_keys` when the item is about the same attribute.** Mint a new one only when none fits. Never put the value in the key: `user.editor`, not `user.editor.helix`.
7. Set `valid_from` and `valid_to` only when the text gives the dates of an effective window, such as a trip or a contract. Keep every date the user states inside `content`, as an absolute date when the text allows.
8. Write `title` and `content` in the language the user used.
9. Skip pleasantries, the questions themselves, anything true only for one reply, and anything already in `known_notes` unchanged. Return at most `{items}` items, most important first, and `{"items": []}` when there is nothing to keep.

### 2.2 The output schema

```json
{
  "items": [
    {
      "title": "Editor preference",
      "content": "Since June 2026 the user edits with Helix; before that, vim.",
      "kind": "preference",
      "stated_by": "user",
      "confidence": 0.9,
      "rows": [3],
      "aliases": ["Helix", "编辑器", "文本编辑器"],
      "fact_key": "user.editor",
      "replaces": "n1",
      "replace_reason": "The user said they switched from vim to Helix in June.",
      "valid_from": null,
      "valid_to": null
    }
  ]
}
```

| Field | Type | Required | Rule |
|---|---|---|---|
| `title` | string | no (falls back to the content) | 1–80 characters after trimming, cut at 80 |
| `content` | string | **yes** | 1–1,000 characters; longer makes the item invalid |
| `kind` | `NoteKind` | no (missing = `fact`) | an unknown value makes the item invalid (as `extraction.rs:420-423`) |
| `stated_by` | `"user"` \| `"assistant"` | no (missing = assistant) | checked against `rows` (§2.4 rule 5) |
| `confidence` | number 0..1 | no (missing = 0) | below `memory_confidence_threshold` (0.65) the item is dropped; the number is never stored |
| `rows` | [integer or `"R<n>"`], 1–8 | **yes** | row numbers of this chunk, as `3` or `"R3"` (both are accepted, since the transcript labels rows `R3`); context rows are removed, and the rest become `sources` (§2.4 rule 4) |
| `aliases` | [string], 0–8 | no | each trimmed to 1–60 characters, deduplicated case-insensitively, the title removed |
| `fact_key` | string \| null | no | 03 §4 grammar `[a-z0-9][a-z0-9._-]{0,63}`, lowercased; an invalid key becomes null and the item is kept |
| `replaces` | `"n<k>"` \| null | no | a ref from `known_notes`; an unknown ref becomes null and the item is kept (counted `replaces_unknown`) |
| `replace_reason` | string ≤ 200 | only with `replaces` | a missing reason becomes `"Restated in conversation."` |
| `valid_from`, `valid_to` | `YYYY-MM-DD` or RFC 3339, or null | no | date-only `valid_from` is that day 00:00:00Z; date-only `valid_to` is **the next day** 00:00:00Z, so the stated last day is inside the half-open window (03 §4). An unparsable or inverted pair drops both bounds. |

References are short (`n1`…`n20`), not UUIDs. The model cannot invent a real note id, and the mapping `n<k> → (id, revision)` is the extractor's own record of the lookup.

### 2.3 The known-notes lookup

This is a new internal service method: `MemoryService::learning_view(ctx, queries: &[&str], chunk_message_ids: &[i64], budget) -> LearningView`.
- **Who may call it.** Only a context whose `surface == Extraction`; every other context is refused.
- **What it is not.** It is not tool-reachable, records nothing in *recently used*, and publishes nothing.
- **What stays the same.** `Capabilities::EXTRACTION` stays `{read:false, write:true, forget:false}` (`context.rs:67-72`), so no tool path gains a read. §11 records the one-line §7a amendment.
- **Where it runs.** On `spawn_blocking`, like every other learning read (§1.9).

It is deterministic: no model, no randomness. It reads the index and re-reads only the files it lists; it never parses a whole partition.

**Two lookup structures join the index (ruling LR18).** The index's `note` table has no `sources` and no `supersedes` column (`memory_index/mod.rs:319-352`), so without them step 2 and §4.5's per-turn read would each parse the partition, the D9 cost. `memory_index::SCHEMA_VERSION` goes from 1 to **2** (`memory_index/mod.rs:44`), adding:
- a `note.supersedes` column: the JSON list of ids, beside `superseded_by`;
- a `note_source(note_rowid, source_key)` table: one row per `sources` entry (`message:<id>` or `task:<id>`), indexed on `source_key` and cascading like `note_alias`.

A cache at version 1 is invalid on open and rebuilt (`memory_index/mod.rs:495-505`), which is the index's documented path (03 §5). Nothing in 03 §7a freezes the index schema, and the FTS layout is untouched (§6.4, LR16).

1. **Partitions.** The home partition and, when the chunk's rows carry a root, that project's partition (§1.1).
2. **Notes from this window.** These are notes, **active or candidate**, found through `note_source` citing any `message:<id>` of `chunk_message_ids`, with origin `extracted_user` or `extracted_other`. They are flagged `from_this_window`, and they are what the re-run guard in §1.4(b) reads. The active ones come first in `known_notes`. The candidates are not listed to the model, because a candidate is never a replacement target; they serve the guard only.
3. **Search.** Each user-authored row of the chunk yields queries through the same query builder and index search `memory_search` uses (`memory_index/query.rs`, `search.rs`). A query is capped at 512 characters (`parse_query` refuses longer, `memory_index/query.rs:181-184`, `MAX_QUERY_CHARS` at `memory_index/mod.rs:50`), and truncating a long row would hide facts stated late in it. So a row longer than 512 characters is split at whitespace into windows of at most 512, and at most **4** windows per row are searched: the first three and the last. The builder caps each query at 32 terms and drops stop words (03 §7c Q9, Q10). The top 5 hits per query are kept, and the union is ranked by each note's best score.
   - Only notes that are effectively active count, whether or not they are recallable. Candidates, superseded and archived notes are excluded.
   - Each hit is verified by re-reading its file (§5 step 7).
   - At most **20 notes** are listed, each rendered as in §2.1 with a 160-character excerpt, all within `known_bytes`. The extractor keeps the `n<k> → (id, revision)` map; §2.5 asks whether a note it finds by key is in it.
4. **Keys.** These are the distinct `fact_key`s of effectively active notes in those partitions, read from the index rather than a partition parse (`memory_index/mod.rs:352`, the `note_partition_fact_key` index; §6.3 quick win 5). The 150 most recently updated are kept, within 2 KiB of `known_bytes`. *(Rev 3.33, owner decision LQ14: a key held **only** by notes the chat model's `memory_store` wrote — origin `extracted_user` with no `observed_at`, which the extractor alone sets — is not listed and names no target for step 2.5; such a note is still listed among the known notes, with no key on its line. A key a learned or a saved note also holds is offered as before. `memory_index::LEARNING_KEY_HOLDER`.)*
5. **Origin labels.**
   - `user` → `saved-by-you`
   - `extracted_user` → `learned`
   - `extracted_other` → never listed, because it is always a candidate
   - `external` → `edited-by-hand`
   - `imported` → `imported`

   The label is what tells the model a replacement will become a suggestion. The decision itself is the service's (§2.5), never the model's.

### 2.4 Validation, in order

Each numbered rule is deterministic. Its outcome is counted in the flush report, which is logged as `memory learning finished` with every count below.

1. The top level must be `{"items": [...]}`. Anything else is a failed attempt (§1.4). *(Rev 3.33, owner decision LQ13: the answer is read out of a leading `<think>…</think>` block, a trailing `</think>` and prose around it — the last complete top-level value of the answer's shape, else the last — and a bare array of item objects is the items list. A value opened and never closed fails the whole answer, and an answer cut at `max_tokens` is held to the whole-or-fenced `{"items": [...]}` read before, so it is still reported as cut. The same unwrap serves the episode summary (§13.3) and task-output extraction (§2.7), each with its own shape. `task_extraction::parse_json_answer`.)*
2. `content` must be present and at most 1,000 characters. Otherwise the item is `invalid`.
3. `kind` must be valid when present. Otherwise the item is `invalid`.
4. `rows` must name 1–8 rows of this chunk, **at least one of them not a context row**. Context rows are removed from `rows` and never become sources. An item with no row left is `invalid`: every learned note cites its evidence.
5. **Provenance is checked, not trusted (ruling LR3).** `stated_by: "user"` is accepted only when **every** remaining cited row is user-authored (§1.2). An item that also cites an assistant reply, an assistant-written task or a long user row is `OtherOutput`, and so is one that cites none of the user's own rows. This closes F7. It also means text injected into an assistant reply, such as a `web_fetch` summary, cannot ride a user row into an active note, and so cannot automatically replace a learned one (§2.5). The residual, stated plainly: the check proves which rows are cited, not that the content came from them. The model writes the content, and it saw the whole window.
6. `confidence` below the threshold drops the item (`below_threshold`).
7. Aliases are cleaned as in §2.2.
8. `fact_key` is cleaned as in §2.2.
9. `replaces` is resolved as in §2.2.
10. Validity is parsed as in §2.2.
11. **One item per key and per target.** Two items with the same `fact_key`, or the same `replaces`, keep the one whose newest cited row is latest (`duplicate_key`). A tie on that row keeps the one the model listed first. Rule 11 runs again after §2.5 resolves each target, so two items that reach the same T by different routes (one by `replaces`, one by key) are also reduced to one.
12. **Suppressed sources** are removed; an item left with no citation is dropped (`suppressed`, §1.4(d)).
13. **Already learned** (§1.4(b)) drops the item (`already_learned`).
14. At most `items` items survive (§1.6), in the model's order (`over_cap`).

§2.5 adds one more outcome, `unchanged`, for an item that restates its target.

### 2.5 From item to write

`StoreInput` gains `observed_at: Option<DateTime<Utc>>`, accepted only on the `Extraction` surface; the file layer's `NewNote` already carries it (`partition.rs:1015`). It also gains `replace_policy: ReplacePolicy`. The service stops hard-coding `observed_at: None` (`memory/service/mod.rs:1032`).

Every learned note gets:
- `sources` = the cited rows as `{type: message, id, observed_at: <row created_at>}`;
- `observed_at` = the `created_at` of its newest cited row, UTC to the second;
- `aliases`, `fact_key` and validity as validated.

Two notes from one chunk therefore no longer tie on R9, because their `observed_at`s come from different rows (§3.4).

**The target T** of an item is found in this order:
1. its `replaces` ref;
2. else, when it carries `fact_key` K, the R9 winner among effectively active notes with key K, looked up in **every partition of the chunk**: the chunk's project partition first, then the home partition (ruling LR11: "a replacement is an explicit supersede with the same `fact_key`", 03 §6). The `replaces` ref already reaches both partitions, because `known_notes` spans them (§2.3), so the key must too. Otherwise `user.laptop`, a global fact, restated in a project chat would land as a second active note with the same key in the project partition. R9 groups and links exist only within one partition (`authority.rs:177`, `:215-225`), so both notes would stay recalled, which is exactly the owner's "changed preferences pile up";
3. else none.

Then rule 11 runs again over the resolved targets, and rule 13's target clause is checked (§2.4).

**Is it a change? (ruling LR11, second half).** A key alone does not prove that a value changed. Rule 6 tells the model to reuse a key from `known_keys`, which lists up to 150 keys, while `known_notes` shows at most 20 notes. Exact dedup never applies to an item with a target (`partition.rs:971-1001`: it is the branch without `supersedes`), and it could not apply across days anyway, because the body carries the date. So a restatement of an unchanged fact is decided here, deterministically:
1. **`replaces` names T.** The model compared the two and said so: a change.
2. **T was found by key alone and was listed in `known_notes`.** The model saw T and did not call it replaced. For a **saved** T (origin `user`, `external` or `imported`), the item is dropped as `unchanged`, and no suggestion is raised. For a **learned** T, the item is dropped as `unchanged` when its content equals T's `## Knowledge` section after normalisation (NFKC, case-fold, whitespace collapsed, trailing punctuation removed). A note learned before L2 has no such section (today's body is `# {title}\n\n{content}`, `extraction.rs:436-437`), so for it the body below the title is compared. Otherwise it is a change, and the automatic replacement is undoable.
3. **T was found by key alone and was not listed.** The model never saw T, and a change stated in new words is exactly what the lexical lookup misses: "I switched to Helix" shares no term with "The user edits with vim". This case is treated as a change. A suggestion here shows both texts (§3.5, §4.4), so a restatement costs the user one **Keep mine** and never silently retires their note.

The evaluation measures how often case 3 raises a suggestion for an unchanged fact (§7.1 mode `restate`, §7.3).

**Partition.**
- **With T**, the note goes to **T's partition** (ruling LR10). A preference or a `user.*` fact restated in a project chat therefore replaces the global note it changes, instead of being refused as a cross-partition supersede (R4, `memory/service/mod.rs:998-1014`).
- **Without T**, a `fact_key` whose subject is `user` (`user.*`) is written to the **global** partition whatever its kind (ruling LR17). This is 03 §3 rule 2's own reason for making preferences global, "a preference stated in a project chat is still the user's", applied to facts about the user: kind `fact` defaults to the project (`memory_files/note.rs:178-183`), so without this rule "user.laptop" learned in a Lantern chat would be recalled only in Lantern, and restated in Moss it would become a second note there.
- Any other item without T is placed by kind, as today (03 §3 rule 2; `memory/service/mod.rs:979-997`).
- A project fact such as `lantern.staging.db` restated in another project's chat, or in a chat with no project, still finds no T and lands where the chat is. This is stated residue; recall has no cross-partition same-key precedence either (§0.3).

**What is written.** MD-2 is unchanged except for the suggestion row that MD-11 and G2 require:

| Provenance | T | Written as | Why |
|---|---|---|---|
| `UserWords` | none | **active**, origin `extracted_user` | MD-2 |
| `UserWords` | origin `extracted_user` | **active**, `extracted_user`, `supersedes: [T]` (an automatic replacement) | MD-11; honoured, since 4 ≥ 4 (`authority.rs:179`) |
| `UserWords` | origin `user`, `external` or `imported` | **candidate**, origin `extracted_user`, `supersedes: [T]`: a **suggestion** | MD-11 and G2. `external` may be the user's own hand edit of a user note (03 §4), so only a learned target is replaced automatically (ruling LR9) |
| `OtherOutput` | any or none | **candidate**, origin `extracted_other`, `supersedes: [T]` when T: an **inferred suggestion**, confirmable only in the Memory view or the CLI (§4.2 rule 5) | MD-2; a candidate supersedes nothing until confirmed (`authority.rs:171`) |

**The race.** When T's revision moved between the lookup and the write, the service answers `revision_conflict`. The item is dropped (`conflict`) and the chunk still advances, because the change is re-stated the next time the user mentions it.

### 2.6 The note body

The body follows 03 §4's sections, not today's `# {title}\n\n{content}` (`extraction.rs:436-437`):

```markdown
# Editor preference

## Knowledge
Since June 2026 the user edits with Helix; before that, vim.

## Evidence and limits
From what you said on 2026-09-24. Replaces “Editor preference” (learned): The user said they switched from vim to Helix in June.
```

`OtherOutput` reads "Inferred from the conversation on <date>; not said by you." The `Replaces …` sentence is present only when T exists, and it is what the suggestion card and list show as the reason (§3.5, §4.4). The `## Knowledge` section holds exactly the item's content, which is what rule 13 and the change test in §2.5 compare.

### 2.7 Task-output extraction

`extract_task_memories` changes in three ways:
- It adopts `aliases` and `fact_key` through the same item validator (§2.4 rules 7–8), and its prompt gains the `known_keys` line.
- Its items stay candidates (`OtherOutput`) with no `replaces`.
- Its daily gate reads the durable usage (§1.8).

Its cadence and its 500/2,000-character input cut are out of scope (`memory/task_extraction/mod.rs`), because MD-10 governs chat turns.

---

## 3. Replacement (MD-11)

### 3.1 The rule, case by case

| Case | What happens | What the user sees |
|---|---|---|
| **Learned replaces learned** | Automatic: B is active `extracted_user` with `supersedes: [A]`, A is derived `superseded` and out of recall | *Recently learned*: "Learned from what you said · replaced “A” · **Undo**". Undo brings A back (§3.3). |
| **Learned vs a note the user saved** | Suggestion: B is a candidate `extracted_user` with `supersedes: [X]`. X stays active and recalled; B is excluded from recall (candidates never are, 03 §7c Q10). | Memory view → **Suggestions**: "Replace “X” (saved by you) with “B”?" with the reason, **Replace** or **Keep mine**; CLI `openalpaca memory suggestions`; in chat, the card (§4) |
| **Learned vs a hand-edited (`external`) or `imported` note** | As the row above (ruling LR9) | as above, "(edited by hand)" |
| **Inferred (assistant or tool output) vs anything** | Candidate `extracted_other` with `supersedes: [T]`, confirmable only in the Memory view or CLI, never in chat: `memory_confirm`, its preview and `<memory_suggestions>` all refuse or skip it (§4.2 rule 5, §4.5) | Suggestions, labelled "inferred from the conversation" |
| **A `ModelTool` `memory_store` with `supersedes`** | *Amended by rev 3.33 (owner decision LQ11; was: allowed against `extracted_user` and below, `authority_conflict` against `user`).* Against a note the user saved, edited by hand or imported: a **suggestion** — a candidate of origin `extracted_user` with `supersedes: [T]`, written under `SuggestUnlessLearned` (S1–S4), citing the turn's own row the user typed, stored in T's partition when no `scope` is named (LR10); T stays active. The same for a store with no `supersedes` whose `fact_key`'s R9 winner is such a note, and for a `memory_update` edit of one (a suggestion carrying the edited note). Against a learned note, or a conversation summary: replaced as asked, as before. Archive of a `user` note stays `authority_conflict`; forget is unchanged. | Suggestions, "Suggested by Alpaca on <date>" (CLI: "suggested by the assistant"); in chat, the card (§4) on a later attended turn; the tool result tells the model it waits for the user |

### 3.2 Storage and service changes

| # | Change | Where | Why |
|---|---|---|---|
| S1 | A non-user-directed `supersedes` of a `user` note is refused **only when the new note would be active**. A candidate may name any target in its partition. | `memory_files/partition.rs:982-984` | Today even a suggestion cannot be written (D5). A candidate retires nothing while it stays a candidate (`authority.rs:171`), and S9 and S10 close the two ways it could stop being one other than a confirm, so this cannot weaken G2. The existing G2 tests (an active `ModelTool` supersede of a `user` note is `authority_conflict`) stay as they are. |
| S2 | `ReplacePolicy::SuggestUnlessLearned` on `NewNote`, used by the extractor: when the target's derived origin is not `extracted_user`, `create` writes the note with `status: candidate`, and `Created` reports `suggestion: true`. *(Rev 3.33: the condition reads "the target is saved — `user`, `external` or `imported`" (`Origin::is_saved`), the same set for learning, whose targets are never `extracted_other`; a conversation summary named by a chat model's `memory_store` is replaced as asked, as before LQ11. Since LQ11 the chat model's replacements take this policy too.)* The one partition load `create` already does decides it (`partition.rs:969`). | `partition.rs:955-1036`, `memory/service/mod.rs:1015-1021` | Today status comes only from (surface, provenance), and no user-words candidate can exist |
| S3 | A candidate's tombstone carries `supersedes: []`. The file's status is known at forget time. | `partition.rs:1158-1169` | Today a forgotten candidate's tombstone is honoured by rank (`authority.rs:192-202`). A forgotten user-words suggestion (rank 4) would then retire an `external` target (rank 2). Dismissals no longer reach this path: they withdraw (§3.3). |
| S4 | Exact dedup also covers candidates when `supersedes` is set: a pending candidate with equal kind, title, body, key, aliases, tags **and** `supersedes` answers `dedup`. | `partition.rs:985-1001` (dedup today is the no-`supersedes` branch) | A restated change must not mint a second identical suggestion |
| S5 | **Confirm binds the target's revision.** `update(Confirm)` of a candidate that carries `supersedes` takes `target_revision`. A moved or missing target is `revision_conflict` or `not_found`, and the candidate is left as it is. | `partition.rs:1041-1122` (today only the candidate's revision is checked, `:1052-1057`) | A target edited after the suggestion would otherwise be retired unseen |
| S6 | **Confirming a suggestion dismisses every other pending suggestion for the same target**, each as its own single-note **withdrawal** (§3.3: no links, no suppressed sources). They run **after** the confirm has been written, so a confirm that fails dismisses nothing. They run under the same user-directed actor, and the confirm result lists them in `dismissed`. | `memory/service/mutations.rs:18-105` | Several restatements of one change leave several pending suggestions (S4 answers only exact repeats), and after a confirm the others are moot. Each is an independent single-file mutation (03 §6). A dismissal that suppressed its sources would hide the note just confirmed whenever the two cite the same row: the confirm stamps `written_at = now` (`partition.rs:940`), the later tombstone's `forgotten_at` is not earlier, and `source_forgotten` holds when `written_at <= forgotten_at` (`authority.rs:245-253`), which automatic recall filters (`memory_index/search.rs:223`). |
| S7 | `recently_learned` lists only **recallable** notes: it skips same-key losers and `source_forgotten`. Its items gain `supersedes`. | `memory/service/mod.rs:826-829`; `service/wire.rs:91-105` | A learned note that silently loses to a `user` note was listed as learned and never used |
| S8 | List items (`memory_list`, `GET /v1/memories`, `recently-learned`) gain `supersedes: [uuid]` | `service/wire.rs:91-105` | The GUI row must say what B replaced |
| S9 | **A suggestion is never archived.** `update(Archive)` of a candidate that carries `supersedes` is `invalid_input` on every surface ("a suggestion is replaced or kept, not archived"). A candidate without `supersedes` archives as today. | `partition.rs:1078-1091` | Derivation skips a link only while the note is a candidate (`authority.rs:171`). An archived former suggestion's link therefore counts, and archive records the actor's origin: a `ModelTool` archive records `extracted_user` (rank 4), which retires an `external` or `imported` target that LR9 meant to protect, and a raw API archive records `user`, which retires any target with no `target_revision` (S5). |
| S10 | **A model tool never edits a suggestion.** `update(Edit)` of a candidate that carries `supersedes` from a non-user-directed actor is `authority_conflict`, naming the Memory view (P9). | `partition.rs:1092-1106` (today it refuses only a `user` note) | Such an edit would rewrite the text a confirmation card later shows and re-record the candidate as `extracted_user`, which would turn an inferred suggestion into one that §4 treats as the user's words. |

### 3.3 Undo, dismiss and forget

Three acts, two meanings. **Forget** means "forget what I said". **Undo** and **dismiss** mean "keep my note; take back what learning did". They must not share a tombstone.

- **Forget is unchanged.** Its tombstone keeps the note's `supersedes`, so a forgotten replacement keeps retiring what it replaced, and fixtures 16 (steps 10–13) and 24 still pin that (`crates/openalpaca_storage/tests/fixtures/memory/24-forgotten-replacement-not-revived.json`). It suppresses every cited `message:<id>` (`memory_files/partition.rs:1156-1167`), so every note citing the same message becomes `source_forgotten` and leaves automatic recall (`authority.rs:245-253`; fixture 16 pins that for a sibling note, `tests/fixtures/memory/16-forget-suppression-and-replacement.json`). §1.4(d) then keeps the message from being re-learned. That is 03 §6's forget contract, and it is what an explicit forget asks for. A forgotten candidate's tombstone carries `supersedes: []` (S3).
- **Withdraw (ruling LR12).** `Partition::forget` takes a `ForgetMode::{Forget, Withdraw}`. `Withdraw` writes a tombstone with **`supersedes: []` and `suppressed_sources: []`**, so no snapshot schema change is needed. It removes the one note and nothing else: X keeps its place, and every other note the same message taught stays learned and recallable.
  - Take "I switched to Helix, and I moved to Berlin", then a Deny on the editor suggestion. Under a forget, the Berlin note would silently leave recall for good; under a withdrawal it stays.
  - v2 makes shared citations the normal case, because one long user message yields several items. That is why every "reject this change" act withdraws.
- **Undo withdraws.** Undoing B brings back what B replaced. In a chain A ← B ← C, undoing C brings B back, and A stays superseded by B.
  - The precondition **widens** to: origin `extracted_user`, **or** status `candidate` of any origin. Otherwise it is `409 MEMORY_NOT_UNDOABLE` as today (`memory/service/mutations.rs:195-230`). A candidate is always something learning proposed, so undoing one is dismissing it.
- **Dismiss is undo of a suggestion.** Several acts dismiss, and each is the same withdrawal: the Memory view's **Keep mine** and `openalpaca memory undo <id>` (both through the undo route), a **Deny** on the in-chat card (§4.3), and S6's dismissals (both through the service's withdrawal).
  - A suggestion's cited rows are not suppressed. The cursor is already past them, so nothing re-reads them. The one exception is a crash re-run, stated in §1.4's residuals.
  - A user who wants the words themselves gone uses **Forget**, which is unchanged.
- **Restore after a confirmed replacement** is deferred (§0.3). The preview and the toasts therefore say that the old note is "kept as history", never that it can be brought back.

### 3.4 The same-`fact_key` rule and R9

- **A learned write with key K becomes a replacement of K's R9 winner** in either partition of its chunk, unless it only restates it (LR11, §2.5). Same-key conflicts between learned notes therefore arise only from `ModelTool` stores and hand edits.
- **Learned notes carry `observed_at`** (§2.5), so among equal-authority learned notes R9's "newest `observed_at`" now decides (03 §7c R9). Before this, every learned note had `observed_at: None`, and ties in one batch fell to the lowest UUID (`authority.rs:311-313`).
- **A suggestion does not enter the key group**, because candidates are never in a group (`authority.rs:215-223`). X stays the winner until the confirm. After the confirm, B is `user` and links to X, so X is superseded and leaves the group, with no conflict row.
- **R9's documented consequence stays true.** A confirmed candidate keeps its `created_at`, so it loses a same-key tie to a `user` note written between the suggestion and the confirmation (03 §7c R9). S5 makes this visible: that newer user note must itself be the target for the confirm to succeed. When the target is an older note, the explicit link decides and R9 is not consulted.

### 3.5 Surfaces

- **API.** `GET /v1/memories/suggestions?limit` (default 20, max 100) answers `{ "items": [{ "id", "revision", "title", "reason", "created_at", "origin", "scope", "sources", "target": { "id", "revision", "title", "excerpt", "origin", "scope" } }] }`. (Rev 3.24: each item also carries `knowledge_left_out` and its target `excerpt_left_out` — how many characters the 4 KiB and 600-byte cuts left out, 0 when whole — and the card ends a cut text with `... (N more characters not shown)`.)
  - It reads the index (`note.supersedes`, LR18) and verifies each listed file, as recall does. It parses no partition.
  - The list is newest first. Where several suggestions name one target, only the newest is listed, and `older_count` says how many were hidden.
  - `POST /v1/memories/{id}` with `action: confirm` takes `target_revision` in the body. It is **required** when the candidate carries `supersedes` (`400 MEMORY_INVALID_INPUT` otherwise) and checked by S5. The result gains `superseded` and `dismissed`.
  - `POST /v1/memories/{id}/undo` also accepts a candidate (§3.3).
  - `docs/api/` is regenerated.
- **GUI** (`apps/openalpaca-gui/src/views/memory/`):
  - **`Suggestions.tsx`** sits above `RecentlyLearned` in `MemoryView.tsx`. Each row reads "Replace “X” (saved by you) with “B”?", shows **both texts** (X's excerpt and B's knowledge), and gives the reason and "From what you said on <date>". Both texts matter because §2.5 case 3 can raise a suggestion that only restates X.
    - **Replace** sends confirm with `If-Match` set to B's revision and `target_revision`.
    - **Keep mine** sends `POST …/{B}/undo` with `If-Match` set to B's revision: a withdrawal, which forgets nothing else the user said (§3.3).
    - The toasts read "Replaced — “X” is kept as history" and "Kept your note".
    - The section hides when empty.
  - **Candidate copy follows the origin** (`views/memory/copy.ts:41-43`, which says "not said by you" today): an `extracted_user` suggestion reads "From what you said — would replace “X”".
  - **A replacement row in *recently learned*** reads "Learned from what you said · replaced “A” · Undo". Its undo toast reads "Undone — “A” is back".
  - New hooks `useSuggestions`, `useConfirmSuggestion` and `useDismissSuggestion` (`hooks/useMemories.ts`), and a new query key `qk.memory.suggestions()`. `memory_changed` already invalidates `["memory"]` (`lib/query-client.ts:181-182`).
  - DESIGN_SPEC §5.7 gains the section.
- **CLI.**
  - `openalpaca memory suggestions [--global|--project <p>] [--limit N]` prints `<id8>  replace “X” (saved by you) → “B”  from your words, 2026-09-24 — <reason>`.
  - `openalpaca memory confirm <id>` fetches the candidate and, when it carries `supersedes`, its target's current revision. It sends both and prints what was replaced and dismissed.
  - `openalpaca memory undo <id>` dismisses a suggestion (§3.3). `openalpaca memory forget <id>` keeps its meaning.
  - `docs/CLI_Manual.md` gains these.

---

## 4. Confirming a suggestion in chat (MD-12)

### 4.1 The mechanism: the model asks, and the human's card answers

**Rejected — Option N, a card the daemon raises when extraction finds a suggestion.** Five reasons:
- MD-10's triggers fire when the user is idle, switching away or stopping, so the card would arrive outside any turn. With no `stream_id` it reaches no SSE, and the CLI's `chat` never sees it (`apps/openalpaca/src/chat_stream/mod.rs:436-441`, `apps/openalpacad/src/event_bridge.rs:443-452`).
- A card a window accepts blocks that window's composer (`apps/openalpaca-gui/src/lib/chat-stream.ts:313-315`). Every window accepts a `lane_key: null` card (`views/chat/useChatSession.ts:118-133`).
- A shutdown flush cannot wait for an answer.
- The request is tool-shaped (`crates/openalpaca_core/src/security/confirmation.rs:17-34`), so every consumer would print a pseudo-tool.
- It does not match MD-12's "the assistant may raise a suggested replacement".

**Adopted — Option T.** A new builtin **`memory_confirm`**, in `tools/builtins/memory_tools.rs`:

| Property | Type | Required | Notes |
|---|---|---|---|
| `id` | uuid | yes | the suggestion: a candidate of origin `extracted_user` with non-empty `supersedes` and at least one user-authored cited row (§4.2 rule 5) |
| `expected_revision` | revision | yes | the suggestion's revision, as listed |
| `target_revision` | revision | yes | the target's revision, as listed (S5) |

- **Result:** `{ "id", "revision", "status": "active", "superseded": [uuid], "dismissed": [uuid] }`. Refusals: `revision_conflict`, `not_found`, `invalid_input` (a target no longer in force — nothing is left to replace — or a malformed revision; rev 3.23, as built in `memory/service/in_chat.rs`: `suggestion_preview` and `check_revisions`), `not_a_suggestion` and `approval_required`; all but `approval_required` before any card. The schema is closed (`additionalProperties: false`).
- **Annotations:** `destructive_local_annotations()`, so it prompts on an attended turn and is refused unattended (`tools/builtins/mod.rs:587-599`).
- **Where it is offered.** Only on the **main loop's per-request surface** (`tools/builtins/main_loop.rs:164-190`), and only when the turn is attended and `ctx.source == Some("gui")`, the derived source the GUI and CLI chat share (`chat/service.rs:193`, `gateway/router/mod.rs:511-520`). It is never offered on connector lanes, unattended turns, the skill tier, the lead or subagents (ruling LR13).
- **Its description** tells the model to call it only for a suggestion listed in `<memory_suggestions>` (§4.5) on this turn, when the conversation reaches the topic or a natural pause, at most once per turn: once its card has been shown, whatever the answer, that suggestion and every other suggestion for the same note leave the list in every conversation on that chat channel (the lane) until OpenAlpaca restarts, so it never raises one that is not listed now, even one it saw earlier (rev 3.22, residue R3: the rule is per lane, `MemoryService::mark_raised`, not per conversation). An inferred suggestion (origin `extracted_other`) is never listed there, and the tool refuses one with `not_a_suggestion`, naming the Memory view and `openalpaca memory confirm`.

`memory_update action: confirm` stays refused from a tool (G2). Today it raises a card and then refuses after the click (`builtins/mod.rs:596`, `partition.rs:1061-1068`), which wastes the click. §4.4's hook now refuses it **before** any card, with a message naming `memory_confirm` for a suggestion and the Memory view otherwise.

### 4.2 Why the human's Approve, and not the model's call, mints `user`

The sandbox can let a prompted call through with no click at all:
- on an approval-cache hit (`security/sandbox/mod.rs:274-281`);
- under `auto_approve` (`:282-307`), which comes from `[security] auto_approve_confirmations` (`orchestrator/query_handler/simple_query_handler.rs:335`);
- and "Always allow" caches the whole tool (`security/confirmation.rs:198-208`).

So "the tool ran after a prompt" proves nothing. Six rules make the click the only thing that can confirm:

1. **The approval is a fact only one branch creates.** `ToolContext` gains `human_approval: Option<HumanApproval { request_id: String }>` (`tools/registry/mod.rs:57`).
   - `SandboxManager::execute_tool` sets it **only** in the `ConfirmationResolution::Answered(resp) if resp.approved` branch (`sandbox/mod.rs:401-411`).
   - It writes it onto the per-call context it finalizes before dispatch (`:460-470`), and **always overwrites** the field there. A caller that builds a context with it set gets `None`.
   - It is never set on the cache-hit, `auto_approve`, unattended or no-broker branches (`:274-341`, `:427-441`).
   - The model cannot supply it: tool schemas are closed, and `ToolContext` is not derived from arguments.
2. **The approval is per call and never reused (ruling LR14).**
   - `tools/builtins/mod.rs` gains `approval_is_per_call(name) -> bool`, which is true only for `memory_confirm`. It sits beside `annotations_for_builtin` and is static per tool name, like every annotation (03 §7).
   - For such a tool the sandbox **skips the cache-hit and `auto_approve` branches** and **never records** into `ApprovalCache` (`sandbox/mod.rs:408-409`). An owner who set `auto_approve_confirmations` still gets the card: approving everything blindly must not mint `user` notes.
   - `ToolConfirmationRequested` carries `per_call: true`, so the GUI offers no **Always allow** (`components/chat/Composer.tsx:291-303`) and the CLI refuses `--entire-tool` for it (`commands/confirmations.rs:62-78`). The server ignores the scope anyway.
3. **Only a user-directed channel can deliver the Approve.** The card rides the turn's `{owner}:gui` lane. Connectors queue only confirmations whose lane ends in their own suffix (`crates/openalpaca_connectors/src/telegram/connector.rs:143-151`, `discord/connector.rs:234-240`, `imessage/connector.rs:198-204`), and their `/yes` pops only their own chat's queue (`common/mod.rs:110-172`). So the one way to answer is `POST /v1/chat/confirmations/{id}`, which builds the response server-side (`apps/openalpacad/src/routes/chat.rs:873-890`). The callers are the GUI card, the CLI's inline `[y/N]` and `openalpaca tasks confirmations approve`. That is the HTTP API, the CLI and the GUI, which is exactly 03 §4's user-directed surface.
4. **The args bind what the human saw.** `args_hash` covers `id`, `expected_revision` and `target_revision` (`sandbox/mod.rs:268`). The preview (§4.4) is computed from exactly those revisions, and the service re-checks both (S5).
5. **The service maps only this one act.** `MemoryService::confirm_suggestion(ctx, approval: &HumanApproval, id, expected_revision, target_revision)` requires:
   - a `ModelTool` context;
   - an approval;
   - a candidate that carries `supersedes`, of origin **`extracted_user`**, with at least one cited `message` row that is user-authored (§1.2; one indexed lookup of the cited ids).

   Anything else is `not_a_suggestion`. An inferred candidate (origin `extracted_other`: text from assistant replies and tools) is confirmable only in the Memory view and the CLI, so an in-chat card never presents the assistant's inference as the user's words. S10 keeps a model tool from editing a suggestion, so the text the card shows is the extractor's.

   It performs the confirm as `Actor::UserSurface` for that one write, so origin is `user` and `memory_changed.surface` is `"user"`. It then runs S6's dismissals, as withdrawals after the write. Every other call from the same context stays `ModelTool`: `edit`, `archive` and `store` against a `user` note are still `authority_conflict` (`partition.rs:1078-1107`).
6. **The model saying yes changes nothing.** Without the fact, `memory_confirm` answers `approval_required` and writes nothing. This is what a turn run with `auto_approve` would reach, if rule 2 did not already prevent it.

03 §4's definition therefore gains one sentence (§11): *a confirm of a suggestion whose approval is the human's Approve on the confirmation card for that exact call, delivered through `POST /v1/chat/confirmations/{id}`, is a user-directed act.*

### 4.3 Deny, timeout, cancel, unattended, connectors

- **Deny** (recommended answer to LQ2): the suggestion is **dismissed durably**.
  - `BuiltInTool` gains an optional hook, `on_confirmation_denied(&self, args, ctx)`, whose default does nothing (`tools/registry/mod.rs:222-230`).
  - The sandbox calls it in the `Answered` not-approved branch, before returning `Err` (`sandbox/mod.rs:412-416`), with `ctx.human_denial = Some(..)` set under rule 1's discipline.
  - `memory_confirm` implements it as a **withdrawal** of the candidate (§3.3): no links and no suppressed sources, so what else the same message taught stays learned. The model gets "The user declined; the suggestion was dismissed. Do not raise it again." X is untouched.
  - If the owner picks LQ2(b), the hook is not built. Deny then leaves the candidate in the Memory view, and the lane's `raised` set (§4.5) keeps it out of this conversation.
- **Timeout** (`confirmation_timeout_secs`, 300 s) and **Cancelled**: the candidate is left as it is, and the model gets the sandbox's existing message (`sandbox/mod.rs:417-425`). The suggestion is not raised again on that lane until the daemon restarts (§4.5).
- **Unattended turns** (scheduled skills, piped or one-shot `chat`, flagged follow-ups): `memory_confirm` is not on the surface. The sandbox would refuse it anyway (`sandbox/mod.rs:308-340`). The suggestion waits in the Memory view and the CLI, which is MD-12's "the Memory view and the CLI keep it too".
- **Connectors** (Telegram, Discord, iMessage): the tool is not offered and `<memory_suggestions>` is not rendered. A connector's `/yes` is keyed by chat and not by sender, and may land in a group (MD-4). It is not a user-directed surface by 03 §4's definition (G2). Connector-only users see suggestions in the app's Memory view.

### 4.4 The preview on the card

Today the card, the CLI prompt and the connector prompt show the literal arguments (`components/chat/ToolConfirmationBanner.tsx:9-10`, `chat_stream/mod.rs:462-475`). For `memory_confirm` those are three opaque ids. A title passed as an argument would be model-authored and could misdescribe the note. So the daemon writes the text.

- **Hook.** `BuiltInTool` gains `confirmation_preview(&self, args, ctx) -> Result<Option<String>, String>`, whose default is `Ok(None)`. The sandbox calls it in the broker branch before `broker.request` (`sandbox/mod.rs:341-359`):
  - `Err(reason)` returns `Err(reason)` with **no card**;
  - `Ok(Some(text))` attaches the preview.
- **`memory_confirm`'s preview.** It validates the suggestion and the target at the given revisions under §4.2 rule 5's conditions, and on a mismatch or a non-suggestion it refuses before any card. It renders at most 1 KiB, from the service's own read:

  ```text
  Replace your saved note “Editor preference” with “Editor preference”?
  Yours: I use vim for everything.
  New: Since June 2026 the user edits with Helix; before that, vim.
  Why: The user said they switched from vim to Helix in June. (from what you said on 2026-09-24)
  Your note is kept as history in the Memory view.
  ```

  The date in "(from what you said on …)" is the `created_at` of the newest **user-authored** row the suggestion cites, looked up from its `sources`, never fixed text. Rule 5 guarantees there is one.

  **As built since the approval-surface hidden-character work (rev 3.24, 3.25; the spec's amendments A18, A19, A22, A32):**
  - **What it quotes.** The five fields the Memory view's Suggestions card serves (§3.5): the two titles and the reason whole, the replaced note's text as its **600-byte excerpt** (`target_excerpt`, quoted after "Yours:" or "Current:") and the suggestion's as its **4 KiB knowledge** (`suggested_knowledge`). A quote the daemon's own cut shortened ends with "…" even where what is left fits the preview's budget. Each field is cut to its own budget: titles 80 bytes, the reason 160, each text 260.
  - **What it drops.** Every character of the approval surfaces' escape set, `utils::text::needs_escape` (control, format, default-ignorable, unassigned and private-use code points, every blank but the plain space, U+2800, U+FFFC), through `utils::text::one_line`: the preview stays OpenAlpaca's prose, composed to be read.
  - **The count line.** When the five fields held any such character, a last line says so, counted over the whole of each served field by `memory::service::suggestion_hidden` — the count the Suggestions card and `openalpaca memory suggestions` show: `This preview leaves out N hidden character(s); the Memory view shows them as <U+…>.`
  - **The budget under the count.** With the count line present the two texts take 216 bytes each instead of 260 (`PREVIEW_TEXT_BYTES_UNDER_COUNT`), so the rest is at most 925 bytes for the longest wording ("your note (edited by hand)") and no cut reaches the date, the last line or the count at any count below 10^16; a preview that left nothing out is unchanged. `the_preview_keeps_its_tail_at_its_worst_case` pins that case.

- **`memory_update`'s preview.** With `action: confirm` it returns `Err(confirm_refused …)`, which fixes the wasted click (§4.1).
- **Wire.**
  - `ConfirmationRequest`, `SystemEvent::ToolConfirmationRequested`, `ServerEvent::ToolConfirmationRequested`, SSE `ConfirmationRequested` and `PendingConfirmation` each gain optional `preview` and `per_call` (`events.rs:338-370`; `crates/openalpaca_api/src/events/mod.rs:239-266`; `chat/stream_manager/mod.rs:58`; `routes/chat_types.rs:172-189`). The bridge's and persistence's patterns already end in `..` (`event_bridge.rs:420-429`, `events/persistence.rs:301-306`). The literals that build these events, and the one pattern without `..` (the SSE serializer, `routes/chat.rs:604-615`), are listed in §8.10.
  - **`event_log` keeps ids only** (`events/persistence.rs:301-321`). A test pins that the preview is never persisted: a later-forgotten note's title must not survive there (03 §6).
  - The unscoped pending read `GET /v1/chat/confirmations` does carry the preview. That widens T22 (pending) with note text under the same bearer; §10 notes it.
- **GUI.** `ToolConfirmationBanner` shows the preview above the literal arguments, labelled "From OpenAlpaca", and the arguments stay available. With `per_call` the composer drops **Always allow**. DESIGN_SPEC §3.14's "literal arguments, never summarised" rule gains the exception: *a daemon-authored preview, never the model's text*.
- **CLI.** The inline prompt and `tasks confirmations list|watch` print the preview before the JSON arguments.

### 4.5 How the assistant learns there is a suggestion

On main-loop turns where `memory_confirm` is offered, the per-turn memory section appends a `<memory_suggestions>` block. It is built with the recall, outside the cached layer (03 §5), and delivered as quoted data under the trust label:
- It holds at most **3** pending suggestions, newest first, one line each, in at most 1 KiB, and that counts against the 8 KiB memory budget.
- Each line reads `[memory-suggestion id=<B> revision=<rB> replaces=<X> replaces_revision=<rX>] “<X title>” (saved by you) → “<B title>” (from what you said on <date>)`, with the date taken as in §4.4.
- Only suggestions that `memory_confirm` would accept are listed: origin `extracted_user` and a user-authored cited row (§4.2 rule 5). An inferred suggestion never appears.
- A suggestion already **raised on this lane** since the daemon started is left out. The queue keeps that set in memory, and `memory_confirm`'s preview hook adds to it. A timed-out or denied card is not re-raised in the same conversation.
- The block carries no instructions. When to call is in `memory_confirm`'s description (§4.1), because instructions inside quoted data are ignored by design.

It is read by a new `MemoryService::pending_suggestions(ctx, limit)`, which reads with the turn's `ModelTool` read context. It is not a recall, so it adds nothing to `memory_used`.

**Its cost is on the reply path, so it reads the index, not the partition.** Candidates and `supersedes` are reachable today only through `Partition::load()` (`memory/service/mod.rs:751-757`, `:821-825`), while recall uses the index plus verification (`memory/service/recall.rs:172-220`). A parse on every attended GUI turn would grow with the note count. `pending_suggestions` therefore selects `effective_status = 'candidate' AND supersedes <> '[]' AND origin = 'extracted_user'` from the index's new `supersedes` column (LR18, §2.3), newest first. It then re-reads files until three qualify, reading at most ten. The service bench measures it (§6.4).

---

## 5. Search for every agent (`memory_read`)

Six shipped templates grant `memory_read`: `lead_agent`, `general_agent`, `code_agent`, `planning_agent`, `review_agent` and `explore_agent` (`config/agents/lead_agent.md:8` and so on). **Three do not:** `writing_agent`, `research_agent` and `system_agent` (`config/agents/research_agent.md:5-9`). None denies it. The design is written for the recommended answer to LQ3:

- **Shipped templates.** The three missing templates list `memory_read`, so all nine name it.
- **An ambient grant covers the owner's install without a re-seed.** The owner's `config/agents/` is never re-seeded (`apps/openalpacad/src/bootstrap/content.rs:102-108`), and a factory reset does not touch it. So `AgentTemplate::to_subagent` appends `memory_read` to `capabilities` and `allowed_capabilities` for every **internal** template (`AgentSource::Internal`, `agent/template/mod.rs:21-28`, `:92`) that neither lists it nor names it in `denied_capabilities`.
  - This is exactly the `workspace_read`/`workspace_write` precedent (`crates/openalpaca_core/src/agent/template/mod.rs:501-521`).
  - `denied_capabilities` still wins (`tools/registry/mod.rs:894-900`).
  - **A plugin-contributed template (`AgentSource::Plugin`) gets `memory_read` only when it names it.** A plugin-backed subagent runs as the run's owner (`runner/lead_agent/tools.rs:343`). Its proxied tool calls are checked against the same widened policy (`tools.rs:575-581`), and `run_plugin_agent_loop` sends each result back to the plugin's process (`runner/plugin_agent.rs:241-247`). An ambient grant would therefore hand every note the owner has to third-party code by default. The memory read tools never prompt (`tools/builtins/mod.rs:588-590`), so no card would stand in the way. A plugin template that names `memory_read` keeps getting it, exactly as today.
- **Ownership is unchanged.** A subagent's reads use the run's owner (`runner/lead_agent/tools.rs:338-358`), and a `system` run refuses (03 §3).
- **Exposure.** Notes arrive as trust-labelled quoted data. `research_agent` also has `web_access`, as `general_agent` and `planning_agent` already do.
- **Cost.** Three read-only tool definitions per prompt, which never prompt (`builtins/mod.rs:588-590`). They count against small budgets such as research's 12 tool calls and 8 rounds (`research_agent.md:19-22`), but a call is spent only when the agent chooses to search.
- **What 03 must say.** §7's "**no agent template changes**" (line 359) is revised (§11).

---

## 6. Smaller items

### 6.1 Validity dates in the recall header

`RecalledNote` and `VerifiedItem` gain `valid_from`, `valid_to` and `timing: Option<Timing::Upcoming>`. The bounds are filled from the re-read `Note` at both construction sites (`memory/service/recall.rs:254-272`, `:363-413`, `:565-581`); `timing` comes from the index (below). `render` (`types.rs:385-417`) appends ` valid_from=<ts|-> valid_to=<ts|->` **only when a bound is set**, followed by ` timing=upcoming` when the index says the window has not begun:

```text
[memory id=… revision=… path=notes/…md kind=episode scope=global heading="" lines=1-6 match=title freshness=verified sources=- reason=recall valid_from=2026-10-05T00:00:00Z valid_to=2026-10-20T00:00:00Z timing=upcoming]
```

- **Unwindowed notes.** They render byte-for-byte as today, so no existing expectation moves (`memory/service/tests.rs:1766-1781`, `prompt_ctx/sources/memory.rs:364`).
- **Ended windows.** These never reach automatic recall (`memory_index/search.rs:186-228`, `memory_index/mod.rs:729-757`), so `upcoming` is the only non-current value the header carries.
- **Clock.** "Upcoming" must use the index's clock, so the header and the filter agree. `MemoryIndex::now` is private to the storage crate (`memory_index/mod.rs:527-529`), and the header is built in core, so no visibility change could reach it. Instead each `SearchHit` and pinned row the index returns gains `upcoming: bool`, computed in the same query as the validity filter (`valid_from > instant`, `memory_index/search.rs:186-228`) and on the same clock. No new clock API is needed.
- **Trust label.** `TRUST_LABEL` (`recall.rs:21`) gains: "`valid_from`/`valid_to` are a note's effective window (`valid_to` exclusive); `timing=upcoming` means the window has not begun, so the note describes a plan, not the present."
- **Search results.** For parity, `memory_search` results gain `valid_from` and `valid_to` (RFC 3339 or null; §11).
- **What is not shown.** `observed_at` and the title are not added to the header. The owner asked for validity. `observed_at` is now set for learned notes (§2.5), so a later revision can add it.

### 6.2 A subagent's notes in *recently used*

- **How a subagent reaches memory.** Only through its own `memory_search` and `memory_read` calls, and those already reach the ring with its `task_id`. `ToolContext.task_id` is set for every subagent (`runner/lead_agent/tools.rs:342`), `context_for_tool` copies it into the `Attribution` (`memory/service/mod.rs:302-325`), and a `ModelTool` search records what it returned (`recall.rs:530-543`; `read` records at `memory/service/mod.rs:700`). §5 gives every internal template those tools, which is what makes a subagent's notes reach *recently used*.
- **The other path carries nothing.** A distilled package section would carry memory into a subagent's prompt, and it would not be recorded: `distill` drops the section key (`prompt_ctx/manager.rs:149-154`) and `to_bundle` re-keys it `Package` (`prompt_ctx/package.rs:120`). But the lead's parent bundle is `ContextBundle::empty()` (`runner/lead_agent/mod.rs:229-231`), and the spawn path says so (`tools.rs:427-430`). No production run puts memory in a package today.
- **So nothing is edited (ruling LR15).** One test pins the owner's ask on the real path: a subagent's `memory_search` produces a ring row carrying its `task_id`. The package path is recorded as residue: whoever first fills the lead's parent bundle with memory must key the section and record its delivery then. Rev 1's three edits (`PackageSection.memory_used`, the `to_bundle` key, `SpawnSubagentTool::with_memory_service`) would have changed no production behaviour, and they are dropped.

### 6.3 Parsed partitions: five quick wins, then a cache

**Quick wins.** Each is independently testable and changes no output:
1. `Partition::reconcile`'s "gone" check uses `HashSet`s and becomes O(N), where today it is O(N²) (`partition.rs:1358-1363`).
2. `forget` does one `load_with` per call, not one per target (`partition.rs:1140-1141`).
3. `commit_write` returns the post-write view, so `refresh_navigation`, the service's `reindex` and `update`'s origin read-back reuse it (`partition.rs:887-950`, `:1384-1389`; `memory/service/mutations.rs:79-87`). Full parses per call go from 3 to 1 for `store` and from 5–6 to 2 for `update`.
4. `reindex(full = false)` builds `IndexRecord`s, and clones bodies, only for notes whose fingerprint changed (`memory/service/mod.rs:462-515`).
5. Existing `fact_key`s come from the index, not from a partition parse (§2.3).

**The cache.**
- **Shape.** `memory_files::NoteParseCache` is owned by the service beside `fingerprints` and `last_scans` (`memory/service/mod.rs:155`, `:160`). It is handed to `Partition::open_*` and consulted in `parse_all` (`partition.rs:682-734`).
  - **Key:** `(partition_key, file_name)`.
  - **Valid while** the file's `(mtime_ns, size, inode, ctime)` are unchanged. These are the stat identity the reconciliation scan uses (`memory/service/mod.rs:1214-1224`), plus `ctime`, which no user tool can set back.
  - **Value:** `Arc<(Note, revision)>`, the full parsed note, so `Note`, `NoteView` and every consumer stay as they are (`memory_files/note.rs:217-238`).
- **Bound.** The cache holds at most **128 MiB**, the constant `NOTE_PARSE_CACHE_BYTES`, accounted as body bytes plus metadata. It works all-or-nothing per partition, with least-recently-used eviction across partitions. A partition that would not fit is parsed uncached, as today. Per-file LRU is avoided because a full load scans every file, which thrashes it. 128 MiB holds about 70K average notes (1.3 KB body) or about 180K learned-size notes (§6.4 measures both).
- **Snapshot.** The parsed `ControlSnapshot` is cached, keyed by its file stat plus an in-process generation that every `save` bumps (`snapshot.rs:250-258`).
- **Invalidation.**
  - **Service writes** update the cache directly in `commit_write`: lstat the renamed file and insert. A retitle removes the old file name's entry (`partition.rs:916-929`), and `forget` removes entries.
  - **External edits** show up as a stat mismatch, so that one file is re-read. This is 03 §5's "changed files are re-hashed".
  - **A workspace move or an index rebuild** evicts the entries wherever `fingerprints` and `last_scans` are evicted (`memory/service/mod.rs:401-412`, `:623-626`).
  - `doctor --repair` and `adopt` run with the daemon stopped, so no cache can be stale across them.
- **The invariant.** **A mutation re-reads and re-hashes the file it changes before its revision check** (`partition.rs:970-984`, `:1050-1057`, `:1141-1156`). Otherwise a same-size, same-mtime external edit would match a cached revision and be overwritten.
- **Testing the invariant.** A real edit cannot reproduce a same-stat change: writing a file and then setting its mtime back (`File::set_modified`) both move `ctime`, so the cache misses on `ctime` and the test would pass with the invariant reverted. The cache therefore takes its stat function as a parameter, `NoteParseCache::with_stat(fn(&Path) -> StatKey)`, defaulting to the real `lstat`. The test injects one that reports the pre-edit key after an external edit. It then shows that a mutation still re-hashes and refuses, and that the same test fails once the re-hash is removed.

### 6.4 The 100K bench and the index size: measure first

- **Bench.** `memory_index/bench.rs` gains `run_bench(n, mix)`, shared by today's 10K test and a new `#[ignore]`d `bench_search_100k_notes_with_verification`. Both run in release: `cargo test --release -p openalpaca_storage -- --ignored bench_ --nocapture`, the same as the existing test (`bench.rs:1-3`, `:27`, `:336-443`).
- **What each run prints:**
  - build time;
  - cache MiB, with the **`dbstat`** breakdown (`SELECT name, SUM(pgsize) FROM dbstat GROUP BY name`, compiled into the bundled SQLite);
  - search + verification p50/p95/max;
  - the empty-query rate;
  - the machine: `available_parallelism`, `std::env::consts` and the hostname via `Command`, as `memory_eval.rs:1312-1330` does.
- **Mixes.** A second mix, `learned`, has 70 % of notes at 200–400 B, the shape of §2.6's bodies.
- **RSS** is measured outside the process, as `/usr/bin/time -l target/release/deps/openalpaca_storage-<hash> --ignored bench_search_100k --nocapture`, so the compile is not counted.
- **Service-level bench.** A new `#[ignore]`d `memory/service/bench.rs` in core seeds by writing files and building the snapshot in one pass, through `repair`'s path. It does not seed through `create`, which would make seeding quadratic.
  - Sizes: 1K, 10K and 100K notes.
  - What it measures:
    - `open_default` with a valid cache and with a missing one (the boot path, `apps/openalpacad/src/services/mod.rs:218-221`);
    - recall p50/p95, including `touch`'s stat scan;
    - `store`, `update`, `forget`, `list` and `recently_learned` latency;
    - `learning_view` for one chunk and `pending_suggestions`, the one learning read on the reply path (§4.5);
    - the first reconciliation tick;
    - snapshot and `MEMORY.md` sizes.
  - Each is measured with the parse cache on and with it off (`cache_bytes = 0`).
- **Proposed targets, not gates:** recall p95 ≤ 100 ms and learned-`store` p95 ≤ 100 ms at 100K notes on the M5 Max. 04 §5's 50 ms p95 target stays the 10K gate.

**The index layout is not changed in this wave (ruling LR16).** The survey's replica of the index schema reproduced the reported 10K size within 2 % (105.4 MiB against 107.7 MiB). It projects today's layout at **1,020 MiB (8.2×) at 100K**. The trigram index is 48 % of that, and the `chunk` table, which holds the text twice plus title, aliases and tags on every chunk, is 33 %.
- **The one candidate:** trigram only over title, aliases, tags and CJK-bearing chunks, with a `unicode61` prefix fallback; both FTS tables contentless; one copy of the chunk text. The replica gives about 352 MiB (2.8×).
- **Why it waits.** It reverses 03 §5 step 4 and §7c Q9, which keep the trigram companion on every term of three or more characters (G23), so it is a design revision.
- **Its cost on the fixtures:**
  - **21:** step 1 would pass only through the title ("Editor preference") plus the prefix fallback, which would have to be reported as `trigram` unless a `prefix` `MatchReason` is added (§7a is frozen). The fixture's intent, `prefer` inside the body's `prefers`, would then rest on the prefix query, and a body-only variant would be added.
  - **21 and 23:** the index-level `excerpt_contains` checks (`crates/openalpaca_storage/src/memory_index/fixture_tests.rs:568-573`) would need the index runner to read excerpts from the file, as the service already does (`recall.rs:386-393`).
  - **23:** steps 5–6 (`简洁的回答`, `用户喜欢`) keep working, because CJK chunks keep trigram.
  - **22 and 24:** unaffected.
- **Why `detail=none/column` is out.** It breaks every phrase of four or more characters with "phrase queries are not supported (detail!=full)".
- **The decision rule, run in L8:**
  - If the 100K run shows search + verification p95 above 100 ms, the layout revision is written with its measured Rust numbers and taken to the owner.
  - If only the size is large, the owner is shown the size table and the projected alternatives, and decides whether about 1 GiB of disposable cache at 100K notes needs the revision.

---

## 7. The learning evaluation

### 7.1 The conversation corpus

**File.** `scripts/eval/memory/conversations-learning.json`, beside `notes-gold.json`. It is JSON, so `.gitignore` needs nothing. The README runbook gains a section.

**Authorship.**
- It is written by a design session from `notes-gold.json`, the way 04 slice 1 had "the design's author" write v0 of the question set.
- It is **never** written from `questions-heldout.json`, and it is tuned only against `questions-tuning.json`.
- A second session reviews it for coverage and leakage before any run.

**Format:**

```json
{
  "schema": 1,
  "sessions": [
    {
      "id": "cs-001",
      "workspace": null,
      "language": "en",
      "narrative_date": "2026-03-02",
      "turns": [
        { "user": "…", "states": ["<gold note id>"], "mode": "explicit" }
      ]
    }
  ],
  "suggestion_sessions": ["cs-031", "cs-044"]
}
```

- `workspace` is `null`, `"lantern"` or `"moss"`, and a project session runs with the scratch project's path as its workspace.
- `mode` is one of `explicit`, `indirect`, `change`, `correction`, `restate`, `rejection` or `noise`.
- `states` lists the gold ids the turn states, and is empty for noise.

**Content**, drawn from the 83 gold notes (55 English, 28 Chinese; 46 global, 24 Lantern, 13 Moss; every note has a `fact_key`):
- **Coverage.** Every gold note that some non-abstention held-out question relies on is stated in at least one turn. **At least 20** of them are stated only **indirectly**, for example "since I sold the Fit in April I take line 5 every morning".
- **15 changed facts.**
  - The **8 gold pairs** are stated old-then-new, in chronological sessions: the 4 `supersedes` pairs (`user.editor`, `user.laptop`, `user.commute`, `moss.output_dir`) and the 4 same-key conflict pairs (`lantern.staging.db`, `lantern.service.port`, `moss.python.version`, `user.weekly_review`).
  - **At least 2 of the `user.*` pairs state the new value in a project session** while the old value was stated in a session with no project. That exercises the cross-partition key lookup and LR17 (§2.5). Without it, the result would depend on which workspace the corpus author happened to pick.
  - **7 in-conversation corrections** follow a first statement that is not gold with the gold value ("Thursdays — sorry, I meant Tuesdays"), so the final truth is still gold.
- **At least 10 restatements** (`restate`): a later session repeats a fact already stated, with the same value and in different words. At least 4 of them are about the 8 change pairs' *old* notes, so that L-S can check them against notes the user saved (§7.2).
- **At least 10 rejections**: the assistant proposes something and the user turns it down, which is the *rejected/failed actions* category.
- **At least 30 % noise turns**: questions, chit-chat and requests with no durable fact.
- **Language by session**, in the questions' proportions (en 303, zh 125, mixed 42 of 470): about 64 % English, 27 % Chinese and 9 % mixed.
- **Dates are in the text.** A learning run cannot backdate, because every write stamps the wall clock. Every dated gold fact states its dates in the conversation: "I switched to Helix in June", "Stockholm, 5 to 19 October".
- **Size:** 40–60 sessions, 250–350 user turns.

**Validation test.** A new file, `crates/openalpaca_core/tests/memory_eval_corpus.rs`, runs in default CI with no model and no feature. It checks:
- the file parses;
- every relied-on gold note is stated at least once;
- every gold id is real;
- every change pair's old turn precedes its new turn in session order, and at least 2 `user.*` pairs change across a project boundary;
- the leakage check: no held-out question shares an 8-word shingle with any turn, **and** no question's CJK characters share a **12-character shingle** with any turn's. Both are NFKC-normalised and case-folded first, and for the second only Han, kana and Hangul characters are kept. The word check alone cannot see Chinese: 125 of the 470 held-out questions are `zh`, where a whole sentence is one whitespace token. Mixed questions get both checks;
- the language shares are within ±5 pp;
- the turn count is in range.

### 7.2 The harness mode

- **Systems.** `memory_eval.rs` gains systems `L`, `L0` and `L-S`, in the same file. It is 2,072 lines, so the shared data types move to `crates/openalpaca_core/tests/common/eval.rs`.
- **Learning phase, through the front door.** `Orchestrator::handle_message` persists nothing (`gateway/router/mod.rs:267` is where rows are written).
  - The harness builds `Gateway::new(shared, lanes, handler, bus, Some(db))` (`gateway/router/mod.rs:249-265`), with a thin `MessageHandler` over the orchestrator, copying the daemon's ten-line `OrchestratorHandler` (`apps/openalpacad/src/gateway_bridge.rs:73-79`). It adds `with_learning(queue)`.
  - Each corpus session is `ConversationRepository::create_session` then `activate_session` (`repository/conversation/mod.rs:408`, `:439`), which is what "New chat" does. A project session sends the scratch project path.
  - **The queue runs in `Manual` mode (§1.5)**: no sweeper, no quiet rule, and no trigger starts a flush on its own, so reader timing cannot change what is learned when. The harness replays the production triggers deterministically. After every 10th user-authored turn of a session it calls `flush_session` (labelled `turns`), and at the end of each session it calls it again (labelled `switch`). The idle trigger has no deterministic stand-in and is covered by L1's tests.
  - **`flush_session(id)`** runs every chunk of the session in order until its cursor reaches the session's last row, with no `MAX_CHUNKS_PER_FLUSH` cap and no quiet rule. It returns the flush report: counts per §2.4 outcome, calls, tokens and latency. `wait_idle()` returns when no flush runs.
  - **Counting the two learning paths.** The harness subscribes to the bus around each flush and counts `SystemEvent::MemoryChanged` by `surface` (`extraction` against `model_tool`). Today the harness's subscription is per question (`memory_eval.rs:1822-1885`), which never spans a flush. The reader's own `memory_store` calls are counted from its turns' subscriptions as before.
- **Replay mode, for repeatable learning numbers.** The reader answers at its model's default temperature, because the main loop sends `temperature: None` (`runner/agentic_loop/backend.rs:52`, `:69`, `:146`). So the transcript the extractor reads differs from run to run. The learning phase therefore keeps its scratch database. `OPENALPACA_MEMORY_EVAL_LEARN_FROM=<scratch dir>` starts a fresh memory store, resets every cursor to 0 and flushes each stored session in order, with **no reader calls**. The extractor runs at temperature 0.0 (as now, `extraction.rs:232`), so a replay re-measures the learning side on a fixed transcript in about 30 minutes instead of 4 hours.
- **Question phase.** `[orchestrator.learning] enabled = false` is swapped into the `ArcSwap` config, and questions are asked exactly as D asks them: fresh lanes, `handle_message`, no persistence (`memory_eval.rs:1822-1845`). The report records `conditions.learning`, which replaces the hard-coded `extraction_disabled: true` (`memory_eval.rs:1796`) and the `extract_every_n_turns` override (`:1682`).
- **The systems:**
  - **L:** conversations in, learning on.
  - **L0:** conversations in, learning off. Only the model's own `memory_store` calls learn.
  - **L-S**, the MD-11/MD-12 check: the 8 *old* notes of the change pairs are seeded as `user` notes through the existing `UserSurface` seeding path (`memory_eval.rs:359-430`), then `suggestion_sessions` are fed. They include the `restate` turns about those notes.
    - Expected: 8 suggestions naming the right targets, **0** user notes retired, and the number of suggestions raised by restatements, reported per turn.
    - Each suggestion is then confirmed through the service's user-directed confirm with `target_revision`, and each old note must become superseded. The in-chat card itself is covered by the deterministic L4 tests, because the harness runs unattended.
- **A forced re-run check** inside L: after the learning phase, forget one learned note, reset its session's cursor, and flush again. The forgotten note must not come back (§1.4(d)).

### 7.3 What is measured

The learned store is dumped into the report with history and candidates.

| Metric | Definition |
|---|---|
| **Fact coverage** | For each relied-on gold note, a historical `memory_search` on its title and aliases returns at most 5 notes, and the judge answers whether any of them states the gold knowledge. One judge call per gold note, with a **frozen `scripts/eval/memory/coverage-judge-prompt.md`** whose sha256 the report records beside `judge_prompt_sha256`, as it does for `judge-prompt.md`. |
| **Noise** | Active learned notes never chosen as a coverage match, plus the candidates raised |
| **Changed facts** (15) | Per pair, one of: new active and old superseded · new active and old a same-key loser · both active and unlinked · new missing · old never learned. Reported **per pair**, not only as a count. |
| **Restatements** | Per `restate` turn: nothing written (`unchanged` or `already_learned`) · an automatic replacement · a suggestion (§2.5 case 3) · a duplicate note |
| **Aliases** | Share of learned active notes with at least one alias in the other script (CJK against Latin, deterministic), and share with any alias |
| **Fact keys** | Share with a `fact_key`; key reuse within each change pair |
| **QA accuracy** | Per category, L against D′ paired with `compare.py`, and L against L0 |
| **Invariants** (hard) | User notes retired without a confirm: 0. Forgotten notes resurrected by the forced re-run: 0. A violation blocks. |
| **Cost** | Extraction calls, input and output tokens, per-flush latency p50/p95, and flushes by trigger |

**Proposed targets, not release gates unless the owner asks:** coverage ≥ 80 %, changed facts ≥ 12 of 15 correct, other-script alias share ≥ 60 %, restatements raising a suggestion or a duplicate ≤ 2 of the `restate` turns, and L within −10 pp of D′ overall.

**One sample is not a pass.** The learning side is reported as the spread over the live run plus **three replays** (§7.2): minimum, median and maximum for coverage, changed facts, restatements and aliases. A target is read against the median, and the spread is printed beside it. QA accuracy is measured once, on the live run's learned store, because each QA pass costs 470 reader turns.

### 7.4 The judge (owner answer to LQ1: the reader judges, and the report says so)

> **Owner, 2026-09-24 (LQ1):** the judge is the **same model as the reader** — `qwen3.8:27b-mlx` (owner, 2026-09-24: tests involving Ollama use this model). No cloud key, no second download. Every report records `judge_is_reader: true` and the report's text states plainly that the model graded its own answers. The guard below is therefore **not** a refusal: it keeps its routability check (`router.is_routable(judge)`) and its post-call assertion that the judge that answered is the model requested, and it **records** — instead of refusing — a judge equal to the reader or of the reader's family. `compare.py` still refuses to compare two reports graded by different judges unless `--allow-judge-mismatch`. The original rev-2 text follows for the record; where it says "refused", read "recorded".


- **What is installed.** Only one model family is installed on this machine: `qwen3.8` in two builds, `27b-mlx` and `27b-mtp-q8_0` (`~/.ollama/models/manifests/registry.ollama.ai/library/qwen3.8/`). The archived D run used `qwen3.8:27b-mtp-q8_0` as both reader and judge ([08](08-evaluation-report.md) §1). The other build would be the same model grading itself. So "a different model" needs one of the two options in LQ1: a cloud judge from another vendor, or a one-time pull of a local model from a different family.
- **Configuration.**
  - The judge is `OPENALPACA_MEMORY_EVAL_JUDGE_MODEL` (`memory_eval.rs:1080`).
  - Its provider sits in the scratch `llm.toml` with `secret_env`, so no key is written to disk.
  - The reader's fallback chain and `[orchestrator] fallback_models` are empty, so the judge's provider can never become a reader rung.
- **The guard runs before any call.** It asserts:
  - `router.is_routable(judge)` (`crates/openalpaca_llm/src/routing/router/mod.rs:250`);
  - `judge != router.effective_default_model()` (`:320`);
  - `judge != extraction_model` when one is set;
  - the judge's **family**, the id before `:`, differs from the reader's.

  The ladder would otherwise silently substitute the reader, with only a one-time WARN (`router/mod.rs:356-405`). After the first judge call the guard asserts `response.model == judge`, and every row's `reader_models` must name the reader alone.
- **Re-judge mode.** `OPENALPACA_MEMORY_EVAL_REJUDGE=<report.json>` re-grades an archived report's rows. Each row has `question`, `gold_answer`, `answer`, `category` and `expect_abstain` (`memory_eval.rs:1454-1471`). It makes no reader calls, and writes a new report with `rejudged_from`.
- **`compare.py`** (`scripts/eval/memory/compare.py:22-58`) prints both reports' judge models and refuses a mismatch unless `--allow-judge-mismatch` is given.

### 7.5 Runs, after everything is in place (L8)

This follows the owner's instruction, read as follows:
- **What waits:** the live evaluation and the benchmarks run once, after L1–L7 have landed.
- **What does not:** every commit still keeps CI green with its deterministic tests, which is the house rule (04 §1).

If the owner meant the unit tests too, only the "Evidence" rows of §8 move.

On the final build, with the chosen judge:
1. Re-judge the archived D report, which yields the self-grading bias of 94.3 % and judge agreement (raw and Cohen's κ).
2. **D′**: the seeded run 08 left pending, graded by the new judge. It is the baseline L is compared with. **E′** (D′ plus follow-up searches) answers a slice-4 question, not whether learning works. It stays with plan slice 4 and is not part of L8.
3. **L**, **L0** and **L-S**, then three replays of L's learning phase (§7.2).
4. The 10K and 100K index benches, and the service bench at 1K, 10K and 100K.
5. §6.4's layout decision rule.

**Time on the local reader:**

| Run | Estimate |
|---|---|
| D′ | ≈ 2.3 h |
| L | ≈ 4 h (300 turns × ~12 s, plus about 40 flushes × 30–60 s, plus 470 questions) |
| L replays (3) | ≈ 1.5 h (no reader calls) |
| L0 | ≈ 3.3 h |
| L-S | ≈ 0.5 h |
| benches | ≈ 0.3 h |
| **Total** | **about 12 h**, plus the judge's time or egress |

The reader's temperature is not pinned, so each live run is one sample. The replays are what give the learning side a spread (§7.3). The results go to a new `tasks/memory-redesign/10-learning-evaluation.md`, which the `tasks/memory-redesign/*.md` whitelist already covers.

---

## 8. Implementation plan

The order below gives each slice its own commits, CI gates green at each commit, and tests written before each behaviour change. Tests start as characterization against the unchanged code where the behaviour exists at HEAD, and otherwise as a new test shown to fail with the change reverted. The gates are 04 §6's commands:
- `cargo test --offline -p openalpaca_storage`
- `cargo test --offline -p openalpaca_core`
- `cargo test --workspace --exclude openalpaca_gui`
- `cargo clippy --workspace --exclude openalpaca_gui --all-targets`
- `cargo check -p openalpaca_gui`
- `python3 scripts/gen_api_docs.py --check`
- the four `bun run` gates, for GUI commits

| Slice | What lands | Depends on | Size |
|---|---|---|---|
| **L1** | Migration 044 (`request_id`, `workspace_root`, the cursor), the queue and triggers, the window, the budget, the rate-headroom rule, the durable daily gates, the retry and backoff rules, the shutdown drain and boot catch-up (§1) | — | L |
| **L2** | Index schema 2 (`note.supersedes`, `note_source`), the extractor schema and its prompt fixture, the known-notes lookup, validation, the change test, `observed_at`, the note body, task extraction's shared fields (§2) | L1 | M |
| **L3** | Replacement and suggestions: S1–S10, withdraw and the widened undo, the suggestions route, GUI and CLI (§3) | L2 | L |
| **L4** | In-chat confirmation: `memory_confirm`, the approval fact, per-call approvals, the preview and deny hooks, the event fields, `<memory_suggestions>`, the GUI card and the CLI prompt (§4) | L3 | L |
| **L5** | `memory_read` for every internal agent, validity in the header and in search, the subagent recently-used test (§5, §6.1, §6.2) | — (after L1 to avoid merge churn) | S |
| **L6** | The five quick wins, the parse and snapshot caches, the 100K and service benches (§6.3, §6.4) | L3 (both touch `partition.rs`) | M |
| **L7** | The corpus and its validation test; the learning harness mode (`Manual` queue, replay), the judge guard, re-judge mode, the coverage-judge prompt and the `compare.py` check (§7) | L1–L4 | M |
| **L8** | The runs of §7.5 and the report; the index-layout decision | L1–L7 | M (mostly machine time) |

### L1 — cadence (MD-10)

**Files**
- **Storage:**
  - new `crates/openalpaca_storage/src/migrations/044_learning.sql`;
  - `migrations/mod.rs`: the entry `Migration { version: 44, name: "learning", … }`, with `BASELINE_VERSION` staying 43;
  - `models/conversation.rs` and `repository/conversation/mod.rs`: `request_id`, `workspace_root`, `learning_window`, and the cursor delete inside `delete_session`;
  - new `repository/learning_cursor.rs`: `get`, `advance_cas`, `record_failure`, `sessions_with_unlearned_turns`;
  - `repository/llm_usage/mod.rs`: `usage_today`.
- **LLM:** `routing/router/mod.rs`: `LlmRouter::rate_headroom`, read-only, over the existing `TokenBucket::available()` (§1.7).
- **Core:**
  - `gateway/persistence.rs` and `gateway/router/mod.rs`: the `request_id` and `workspace_root` writes, `with_learning`, the turn guard before persistence;
  - new `memory/learning/{mod.rs, queue.rs, window.rs, extractor.rs, tests.rs}`. The extractor in L1 keeps today's item fields plus `rows`, with §2.4 rules 1–6, 12 and 14; rule 13 needs L2's `note_source` and lands there;
  - `orchestrator/extraction.rs` is deleted and its tests move;
  - `orchestrator/handlers.rs:331-396` keeps only the summary;
  - `orchestrator/mod.rs` loses `extraction_turn_counter`;
  - `memory/task_extraction/mod.rs` reads the durable gate;
  - `daemon_config/{orchestrator.rs, validation.rs, tests.rs}`.
- **Daemon:**
  - `main.rs`: build the queue, `with_learning` at `:518`, the drain before `:785`, and the boot catch-up;
  - `background.rs`: the sweeper spawn (idle and switch) beside `:259`. No bus subscriber is added.
- **Configuration:** `config_schema/registry.rs`, `apps/openalpaca/src/commands/daemon_config_cli/mod.rs` and `config/daemon.toml`.
- **Docs:** CLAUDE.md, whose *Storage* section gains head 44 and the new table **in the migration commit** (CLAUDE.md "Keep this file current"); `docs/Daemon_Manual.md` (cadence and keys); `docs/api/` regenerated by **every** commit that changes the schema, a public item or a route, because `gen_api_docs.py --check` is a CI gate.

**Evidence, tests first**
- Characterization at HEAD, pinned and then flipped by the change:
  - `default_cadence_extracts_one_turn_in_five`: four turns through `handle_message` reach the extractor stub 0 times.
  - `a_long_message_is_cut_to_800_chars`: the tail of a 2,000-character message is absent from the prompt.
  - `a_followup_row_mints_an_extracted_user_note`: F7.
- New tests, each failing with its change reverted:
  - `every_persisted_turn_is_learned_at_the_next_flush`
  - `a_long_message_reaches_the_extractor_whole`
  - `the_window_reads_content_never_content_json` (U2 re-expressed: an attachment turn's document text is absent from the prompt)
  - `a_followup_row_is_assistant_written`
  - `notices_post_updates_and_completion_reports_stay_out`
  - `slash_turns_and_their_answers_stay_out`
  - `an_unanswered_in_flight_turn_ends_the_window`
  - `a_crash_before_the_cursor_moves_reruns_without_a_duplicate` (a fault injected after the store loop)
  - `a_cancelled_flush_still_advances_after_its_store_loop` (the cancellation lands while the blocking closure runs)
  - `a_rerun_never_resurrects_a_forgotten_note`
  - `three_invalid_outputs_skip_the_chunk_with_one_warn`
  - `an_unavailable_model_holds_the_window_and_backs_off` (no attempt counted, one WARN per outage; P10)
  - `a_quarantined_partition_holds_the_window_before_any_model_call`
  - `a_cap_leaves_the_window_queued`
  - `the_daily_cap_counts_the_utc_day_from_llm_usage_daily`
  - `a_local_model_flush_yields_to_a_chat_turn`, and `an_openai_provider_on_localhost_yields`
  - `a_keyed_flush_leaves_half_the_rate_budget` (a router whose TPM makes a second chunk overdraw: the second chunk waits, and a chat request made then acquires with no wait)
  - `a_turn_mid_flush_stops_it_at_the_next_chunk`
  - `the_gateway_returns_while_a_flush_is_stuck` (a stub model that never answers)
  - `a_turn_is_in_flight_before_its_row_exists` (a flush reading the window between persistence and the reply stops before the row)
  - `a_projectless_turn_in_a_bound_session_learns_globally`, and `a_chunk_never_mixes_roots`
  - `a_long_user_row_yields_only_candidates`
  - the triggers, under `tokio::time::pause`: `idle_120s_makes_a_session_due`, `the_tenth_user_turn_makes_it_due`, `an_archived_session_with_pending_turns_is_due`, `drain_returns_within_its_budget`, `boot_catchup_finds_unlearned_sessions`
  - the budget: `budget_for_an_8k_window` (7,201 B, 3,086 B, 9 items)
  - storage: `migration_044_seeds_existing_sessions_at_their_last_message` (a 43 database with a session and messages opens at 44 with `extracted_through = MAX(id)`), `the_cursor_advances_only_from_the_expected_value`, `deleting_a_session_deletes_its_cursor`
  - llm: `rate_headroom_reports_the_tightest_key_without_consuming`

**Risk**
- **Migration on the owner's live store.** It is additive, tested on a 43 database, and needs no reset. Once migrated, a 43 build refuses the database through slice 0.5's newer-schema guard (`database/mod.rs:175-185`), so a rollback needs a copy taken first. The L1 notes say so.
- **A hook in the reply path.** It makes three in-memory updates and has a test.
- **Contention with chat.** Handled by the quiet start, the per-chunk re-check, the local yield and the rate-headroom rule.
- **A new read on the router.** `rate_headroom` is read-only and changes no provider table. The paused LLM-providers design (04 §7) is told in §8.10.

### L2 — richer extraction

**Files**
- `memory/learning/extractor.rs` and new `memory/learning/extractor_prompt.txt`: the prompt, schema and validation, and the change test (§2.5);
- new `memory/learning/lookup.rs`;
- `memory/service/{mod.rs, types.rs}`: `learning_view`, `StoreInput.observed_at`, `ReplacePolicy` passed through;
- `memory_files/partition.rs`: `NewNote.replace_policy`, the plumbing only; S2's behaviour lands in L3;
- `memory_index/{mod.rs, search.rs}`: `SCHEMA_VERSION` 2 with `note.supersedes` and `note_source` (LR18), `fact_keys(partitions, limit)`, `notes_citing(partitions, source_keys)`;
- `memory/task_extraction/mod.rs`: aliases and key through the shared validator.

**Evidence**
- Characterization at HEAD: `a_learned_note_has_no_aliases_key_or_observed_at`.
- New:
  - `the_extractor_prompt_matches_its_fixture` (byte for byte)
  - `aliases_and_fact_key_reach_the_note`
  - `an_invalid_fact_key_drops_the_key_not_the_item`
  - `the_prompt_lists_known_keys_and_notes_within_budget`
  - `a_long_row_is_searched_in_512_char_windows`
  - `a_reused_key_is_kept`
  - `an_unknown_replaces_ref_is_ignored`
  - `rows_accept_3_and_r3_and_drop_context_rows`
  - `stated_by_user_citing_an_assistant_row_is_other_output`
  - `observed_at_is_the_newest_cited_rows_time`
  - `a_date_only_valid_to_includes_that_day`
  - `two_items_with_one_key_keep_the_latest`, and `two_items_reaching_one_target_by_different_routes_keep_one`
  - `a_restated_listed_saved_fact_is_unchanged`, `a_learned_restatement_with_equal_content_is_unchanged`, and `a_change_against_an_unlisted_saved_note_is_a_suggestion`
  - `a_global_fact_restated_in_a_project_session_replaces_it`, and `a_user_fact_first_learned_in_a_project_is_global`
  - `already_learned_needs_equal_content_not_only_the_key`
  - `a_rerun_never_duplicates_an_inferred_candidate`
  - `learning_view_finds_this_windows_candidates_through_note_source`
  - `learning_view_refuses_every_surface_but_extraction`
  - `learning_view_records_nothing_in_recently_used`
  - `an_index_at_schema_1_is_rebuilt`
  - `the_note_body_has_knowledge_and_evidence_sections`
  - `task_candidates_carry_aliases_and_fact_key`

**Risk.** Output quality on a local model, which L8 measures. Prompt size on small windows, handled by the budget formula and its test.

### L3 — replacement and suggestions (MD-11)

**Files**
- **Storage:** `memory_files/partition.rs` (S1–S5, S9, S10, `ForgetMode::{Forget, Withdraw}`), `memory_files/authority.rs` (unchanged rules; new tests), and a new fixture `crates/openalpaca_storage/tests/fixtures/memory/25-suggestion-lifecycle.json`, run by the existing runners.
- **Service:** `memory/service/{mod.rs, mutations.rs, wire.rs}` (S6–S8, `suggestions` over the index, the widened `undo`), and `memory/learning/extractor.rs` (the §2.5 decision table).
- **Daemon:** `apps/openalpacad/src/routes/memories.rs` and `router.rs` (the suggestions route, `target_revision`, undo of a candidate).
- **CLI:** `apps/openalpaca/src/commands/memory.rs` (`suggestions`, `confirm` with the target, `undo` of a suggestion).
- **GUI:** `views/memory/{Suggestions.tsx, MemoryView.tsx, RecentlyLearned.tsx, NoteTab.tsx, MemoryDetail.tsx, copy.ts}`, `hooks/useMemories.ts` and `lib/api/`.
- **Docs:** `DESIGN_SPEC.md` §5.7, `docs/GUI_Manual.md`, `docs/CLI_Manual.md`, `docs/api/` and `API_MAP.md`.

**Evidence**
- Characterization at HEAD:
  - `create_refuses_an_extraction_candidate_naming_a_user_note` (`partition.rs:982-984`)
  - `undo_of_a_replacement_leaves_the_old_note_superseded` (§3.3)
  - `a_forgotten_candidates_tombstone_is_honoured_by_rank` (S3)
  - `recently_learned_lists_a_same_key_loser` (S7)
  - `undo_makes_a_sibling_note_source_forgotten` (§3.3: at HEAD undo is a forget)
  - `archiving_a_candidate_activates_its_link` (S9)
  - `a_model_tool_edit_of_a_candidate_records_extracted_user` (S10)

  Each is flipped.
- New:
  - `a_learned_statement_replaces_a_learned_note_automatically`
  - `a_learned_statement_against_a_user_note_is_a_suggestion` (X is still recalled and B is not)
  - `an_external_target_gets_a_suggestion`
  - `a_replacement_is_written_in_its_targets_partition`
  - `same_key_without_replaces_supersedes_the_key_winner`
  - `an_active_model_tool_supersede_of_a_user_note_is_still_authority_conflict` (the G2 regression)
  - `confirm_of_a_suggestion_requires_the_target_revision`
  - `confirm_refuses_a_moved_target_and_leaves_the_candidate`
  - `confirm_dismisses_the_other_suggestions_for_the_target`
  - `confirming_one_of_two_suggestions_sharing_a_row_keeps_it_recallable` (rev 1's S6 would have hidden it)
  - `withdrawals_leave_sibling_notes_recallable`, once each for Keep mine, Deny, S6 and undo: a sibling note citing the same message stays out of `source_forgotten` and in automatic recall
  - `forget_still_suppresses_the_message_for_siblings` (fixture 16's reading, unchanged)
  - `undo_brings_the_replaced_note_back`
  - `undo_of_a_chain_revives_one_step`
  - `undo_dismisses_a_candidate_of_either_origin`
  - `archiving_a_suggestion_is_refused_on_every_surface`
  - `a_model_tool_edit_of_a_suggestion_is_authority_conflict`
  - `a_repeated_identical_suggestion_is_dedup`
  - `list_items_carry_supersedes`
  - route tests for `suggestions` (read from the index), confirm without the target (`400`), with a moved target (`409`), and undo of a candidate
  - GUI tests: Suggestions renders both texts, Replace sends `If-Match` and `target_revision`, Keep mine sends undo, candidate copy follows the origin, the undo copy
  - CLI tests for `suggestions`, `confirm` and `undo` of a suggestion
  - fixtures 16 and 24 unchanged and passing

**Risk.** Loosening a G2 guard. It is loosened for candidates only, the regression test stays, and a candidate retires nothing until a user-directed confirm: S9 and S10 close archive and model edits.

### L4 — in-chat confirmation (MD-12)

**Files**
- **Core:**
  - `tools/registry/mod.rs`: `ToolContext.human_approval` and `human_denial`, and `BuiltInTool::confirmation_preview` and `on_confirmation_denied`. `ToolContext` derives `Default` (`tools/registry/mod.rs:56`), but seven struct literals spell out every field and must gain the two: `runner/lead_agent/tools.rs:338`, `runner/lead_agent/mod.rs:268`, `orchestrator/skill/invocation.rs:748` and `:1252`, `orchestrator/query_handler/simple_query_handler.rs:140`, `tools/builtins/artifact_write.rs:539`, and the test literal `orchestrator/tests.rs:672`. L4's first commit converts them to `..Default::default()` before adding the fields;
  - `tools/builtins/mod.rs`: `approval_is_per_call`, and the annotations for `memory_confirm`;
  - `tools/builtins/memory_tools.rs`: `MemoryConfirmTool`, and `memory_update`'s preview refusal;
  - `tools/builtins/main_loop.rs`: the surface rule;
  - `security/sandbox/mod.rs`: the per-call branch, the preview, the fact, the denial hook;
  - `security/confirmation.rs`: `preview` and `per_call` on the request;
  - `events.rs`, `chat/stream_manager/mod.rs`;
  - `memory/service/mutations.rs`: `confirm_suggestion`, `pending_suggestions` (over the index's `supersedes` column from L2; L3's suggestions route shares the query);
  - `prompt_ctx/sources/memory.rs` and `orchestrator/query_handler/simple_query_handler.rs`: the `<memory_suggestions>` block;
  - `memory/learning/queue.rs`: the `raised` set.
- **API and daemon:** `crates/openalpaca_api/src/events/mod.rs`, `apps/openalpacad/src/{event_bridge.rs, events/persistence.rs, routes/chat.rs, routes/chat_types.rs}`.
- **GUI:** `components/chat/{ToolConfirmationBanner.tsx, Composer.tsx}`, `lib/chat-stream.ts` and `views/chat/useChatSession.ts` (the fields).
- **CLI:** `apps/openalpaca/src/chat_stream/mod.rs` and `commands/confirmations.rs`.
- **Docs:** DESIGN_SPEC §3.14, `docs/GUI_Manual.md`, `docs/CLI_Manual.md`, `docs/Daemon_Manual.md`, `docs/api/` and `API_MAP.md`.

**Evidence**
- Characterization at HEAD: `an_approved_memory_update_confirm_is_refused_after_the_click`, the wasted click, flipped to "no card, refused before".
- New, one per branch of the sandbox:
  - `an_approved_memory_confirm_records_origin_user`
  - `auto_approve_still_prompts_a_per_call_tool`
  - `an_entire_tool_approval_is_not_cached_for_memory_confirm`
  - `a_cache_hit_never_carries_an_approval`
  - `a_caller_supplied_human_approval_is_discarded`
  - `unattended_refuses_memory_confirm`
  - `no_broker_refuses_it`
  - `without_an_approval_the_tool_answers_approval_required`
  - `deny_dismisses_the_suggestion_and_leaves_the_target`
  - `timeout_leaves_the_suggestion`
  - `a_stale_target_revision_raises_no_card`
  - `an_inferred_suggestion_is_not_a_suggestion_in_chat` (the tool, the preview and `<memory_suggestions>` each refuse or skip an `extracted_other` candidate)
  - `the_preview_date_comes_from_the_cited_user_row`
  - `memory_confirm_is_absent_on_connector_and_unattended_surfaces`
  - `the_preview_is_daemon_authored_and_bounded`
  - `the_preview_never_reaches_event_log`
  - `the_sse_frame_carries_preview_and_per_call`
  - `memory_suggestions_block_lists_three_and_skips_raised_ones`
  - `pending_suggestions_parses_no_partition` (a test-only parse counter, as in L6)
  - GUI: the banner shows the preview, and there is no Always allow when `per_call`
  - CLI: the inline prompt prints the preview, and `--entire-tool` is refused for a per-call tool

**Risk.** This is a security-sensitive change to the sandbox. The approval fact is set on exactly one branch, and every branch has a test. SystemEvent and ServerEvent gain **no new variant**, so no new `event_bridge.rs` or `persistence.rs` arm is needed (CLAUDE.md *Gotchas*). The **patterns** that read the events already end in `..` (`event_bridge.rs:420-429`, `events/persistence.rs:301-306`, the three connector listeners), but every **struct literal** that builds one must gain the two fields: `security/sandbox/mod.rs:363`, `apps/openalpacad/src/events/handlers.rs:415-433` (the `tool_confirmation_requested` signature and literal, and its callers `event_bridge.rs:434` and the test at `events/persistence.rs:774`), the SSE literal `event_bridge.rs:447`, the test publish at `event_bridge.rs:1134`, and `PendingConfirmation` at `routes/chat.rs:858`. One pattern has no `..`: the SSE serializer `routes/chat.rs:604-615`, which must also emit the two fields (§8.10).

### L5 — search for every agent, the header, subagent deliveries

**Files**
- `config/agents/{writing_agent,research_agent,system_agent}.md`;
- `agent/template/mod.rs`: the ambient grant for internal templates, per LQ3;
- `memory/service/{types.rs, recall.rs, wire.rs}` and `memory_index/{mod.rs, search.rs}` (`upcoming` on each hit and pinned row);
- no `prompt_ctx` or `runner/lead_agent` change (§6.2).
- **Docs:** 03 §7 line 359 (§11).

**Evidence**
- Characterization: `writing_research_and_system_lack_memory_read`, and `an_upcoming_note_renders_no_window`.
- New:
  - `all_nine_templates_name_memory_read`
  - `an_ambient_grant_respects_denied_capabilities`
  - `a_plugin_template_gets_no_ambient_memory_read`
  - `a_windowed_note_renders_both_bounds`
  - `an_unwindowed_header_is_byte_identical`
  - `timing_upcoming_uses_the_index_clock` (an index with a pinned clock marks the hit, and the header prints it)
  - `search_results_carry_validity`
  - `a_subagents_memory_search_records_its_task_id_in_the_ring`
  - `test_golden_lead_agent_spawn_subagent_byte_identical` unchanged

**Risk.** Prompt growth for three templates. The ambient grant is LQ3.

### L6 — parse cache and benches

**Files:** `memory_files/{partition.rs, snapshot.rs}`, new `memory_files/cache.rs` (with the injectable stat function), `memory/service/mod.rs`, `memory_index/bench.rs`, and new `memory/service/bench.rs`.

**Evidence**
- A test-only parse counter on `Partition` pins the quick wins: `store` does 1 parse, `update` 2, and a 20-target `forget` 1 load.
- New:
  - `reconcile_outputs_are_unchanged_on_a_10k_partition`
  - `a_cache_hit_skips_the_read`
  - `a_service_write_updates_the_cache`
  - `an_external_edit_invalidates_one_entry`
  - `a_same_stat_external_edit_is_never_overwritten` (the injected stat function reports the pre-edit key; the mutation's re-hash catches the edit, and the test fails with the re-hash removed, §6.3)
  - `a_partition_over_the_cap_is_parsed_uncached`
  - `a_workspace_move_evicts_the_old_keys`
  - both benches compile under `--no-run`

**Risk.** Serving a stale cached parse to a mutation, which the invariant and its test cover.

### L7 — the evaluation harness and corpus

**Files:** `scripts/eval/memory/{conversations-learning.json, coverage-judge-prompt.md, README.md, compare.py}`, `crates/openalpaca_core/tests/{memory_eval.rs, memory_eval_corpus.rs, common/eval.rs}`, and core's public `LearningQueue::{new(.., QueueMode::Manual), flush_session, flush_all, wait_idle}`.

**Evidence**
- `memory_eval_corpus.rs` runs in default CI, including the CJK leakage check on a planted Chinese shingle.
- Judge-guard unit tests, in the harness's non-ignored helpers: an unroutable judge is refused; a judge equal to the reader or of the reader's family is **accepted and recorded** as `judge_is_reader` / `judge_same_family` (LQ1).
- `flush_session_runs_to_the_last_row_without_the_quiet_rule` and `a_manual_queue_never_flushes_on_its_own` (core, default CI).
- Replay reads a fixture-sized scratch database and learns without a reader call.
- Re-judge reads a fixture-sized archived report.
- `compare.py` refuses mismatched judges, checked by a `--self-test` flag.
- `cargo test -p openalpaca_core --features live-eval --test memory_eval --no-run` passes.

**Risk.** Corpus leakage into the held-out set, handled by the shingle check and the review.

### L8 — runs and report

§7.5 in full. Its output is `10-learning-evaluation.md`, plus the archived reports in `scripts/eval/memory/reports/`, redacted as 08's were, and the layout decision (§6.4). Every run uses `OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR` in a scratch directory. `~/.openalpaca` is never read.

### 8.9 Migration impact

- **One migration, 044, in L1.** It adds `conversation_messages.request_id` and `conversation_messages.workspace_root` and creates `learning_cursor`, seeded at each session's newest message. `BASELINE_VERSION` stays 43 and goes on meaning "oldest openable schema". The index's own `SCHEMA_VERSION` 2 (L2) is not a database migration: the index is a disposable cache, rebuilt on a version mismatch.
- **The five assertions that read a fresh database's version** move to `MIGRATIONS.last().version`: `database/tests.rs:11`, `:34`, `:50` (the whole `schema_version` list), `:375` and `:1036`. The test at `:369` keeps `BASELINE_VERSION == 43`, reads the fresh version from the last migration, and is renamed `a_fresh_database_opens_at_the_newest_version_with_no_memory_tables`. The new table's name keeps it passing (§1.3).
- **No exact column-list assertion exists on `conversation_messages`.** `database/tests.rs:688` checks with `contains`.
- **The generated schema doc** gains the column and the table. `scripts/gen_api_docs.py` already handles `ADD COLUMN` (`:1047`).
- **Rollback.** The owner's live store migrates in place. A pre-L1 build refuses the migrated database rather than misreading it (slice 0.5), so keeping a copy before the first L1 start is the rollback.

### 8.10 Collisions with existing tests and code

| Where | What changes |
|---|---|
| `orchestrator/extraction.rs:586-844` (10 tests) | Move to `memory/learning/tests.rs`. The input becomes a persisted window; the MD-2, cap, threshold, no-owner, cancellation, source, dedup and thinking assertions keep their meaning. |
| `orchestrator/tests.rs:7497-7600` (`extracting_orchestrator`, `a_preference_the_user_typed_becomes_a_note_after_the_turn`, the U2 attachment test) | Drive through a `Gateway` with persistence and a queue flush, because `handle_message` persists nothing |
| `daemon_config/tests.rs:45`, `:54`, `:586-608`; `config_schema/tests.rs:156`, `:367`; `config_schema/registry.rs:308`, `:372`; `daemon_config_cli/mod.rs:103-110`, `:141-143`; `daemon_config_cli/tests.rs:80-82`; `config/daemon.toml:40`, `:45` | Keys removed or renamed (§1.10) |
| `config_schema/tests.rs:101` (`assert_eq!(keys.len(), 43)`) | 48 after L1's config commit adds five rows, 47 once the extractor commit removes `extract_every_n_turns` (§1.10); the comment above it names each change |
| `SystemEvent`/`ServerEvent::ToolConfirmationRequested`, `ChatStreamEvent::ConfirmationRequested`, `PendingConfirmation` literals: `security/sandbox/mod.rs:363`, `events/handlers.rs:415-433`, `event_bridge.rs:434`, `:447`, `:1134`, `events/persistence.rs:774`, `routes/chat.rs:604-615` (a pattern without `..`), `:858` | Gain `preview` and `per_call` in L4 (§4.4) |
| The seven full `ToolContext` literals (L4's file list) | Converted to `..Default::default()` in L4's first commit |
| `memory_index/mod.rs:44` (`SCHEMA_VERSION`) and the index tests that build a v1 cache | Version 2 in L2; a v1 cache is rebuilt |
| `crates/openalpaca_llm` (`routing/router/mod.rs`) | Gains `rate_headroom`, read-only, in L1. The paused LLM-providers design (04 §7) reads no rate-limiter API, and this method changes no provider table. It is recorded there so the redesign keeps it. |
| `crates/openalpaca_core/tests/memory_eval.rs:1682`, `:1796`; `scripts/eval/memory/README.md:113` | `[orchestrator.learning] enabled` replaces the override |
| `database/tests.rs:11`, `:34`, `:50`, `:369-375`, `:1036` | §8.9 |
| Fixtures 16 and 24 | Must pass unchanged (forget semantics) |
| `memory/service/tests.rs:1766-1781`, `prompt_ctx/sources/memory.rs:364` | Unchanged (conditional rendering) |
| `lead_agent/tests.rs` (nine `SpawnSubagentTool::new` sites) | Untouched: §6.2 edits nothing on the spawn path |
| `ToolConfirmationBanner` tests, `useChatSession` tests | New optional fields; the existing cases are unchanged |
| The paused LLM-providers design (04 §7) | The router APIs this design reads are `answering_model`, `is_routable`, `effective_default_model`, `provider_requires_key`, `runtime_config` and the new `rate_headroom` (row above) |

### 8.11 Documents to change, with the slice that lands each

| Document | Change | Slice |
|---|---|---|
| 03 | rev 6 folds §11 | each slice folds its own rows, and the revision log names the slice |
| 04 | A pointer to 09 for the Learning v2 wave. S3-R3 and S3-R5 are marked as taken here. | L1 |
| CLAUDE.md | Schema head 44, `learning_cursor`, the learning cadence | L1 |
| `docs/Daemon_Manual.md`, `docs/CLI_Manual.md`, `docs/GUI_Manual.md`, `apps/openalpaca-gui/DESIGN_SPEC.md`, `API_MAP.md`, `docs/api/` | as listed per slice | per slice |
| `tasks/api-fix-plan.md` §0 | MD-10–MD-12 as taken rows, by the commit that applies each (04 §7's convention) | the applying commit |
| The owner's Obsidian decision log | Mirrored in the same session, per CLAUDE.md | the applying session |

### 8.12 Commit boundaries

- **L1:** seven commits, each building on the one before and green on its own. 04 §1 requires characterization tests written against the unchanged code *before* the change (`04-implementation-plan.md:15`), in commits of their own as slice 3 did (`:345`):
  1. **The migration and repository.** 044, `learning_window`, the cursor repository, `usage_today`, and the cursor delete in `delete_session`. The same commit regenerates `docs/api/` and updates CLAUDE.md's schema head.
  2. **The config keys, with no behaviour.** The `[orchestrator.learning]` struct and its validation, `extract_max_calls_per_day`, and the registry and CLI rows for these and `memory_confidence_threshold`, plus `config/daemon.toml`. Everything is parsed and unread. `extract_every_n_turns` and `extract_max_items_per_turn` stay, because the old extractor still reads them.
  3. **The characterization tests alone**, green at HEAD: `default_cadence_extracts_one_turn_in_five`, `a_long_message_is_cut_to_800_chars`, `a_followup_row_mints_an_extracted_user_note`.
  4. **The `LearningQueue` skeleton and the gateway.** The in-flight set, the per-session counters and `with_learning`, with no flushing; the gateway's `request_id` and `workspace_root` writes; the turn guard before persistence.
  5. **The window and the extractor rewrite.** `flush_session`, the window, the budget, validation rules 1–6, 12 and 14, and the store-and-advance closure. `orchestrator/extraction.rs` is deleted and its tests move. `extract_every_n_turns` is removed and `extract_max_items_per_turn` renamed, with their §8.10 collisions. The three characterization tests flip.
  6. **The triggers and gates.** `rate_headroom` in `openalpaca_llm`, the sweeper (idle and switch), the 10-turn trigger, the stop drain, the boot catch-up, the durable daily gates, and the retry and backoff rules.
  7. **The manuals** (`docs/Daemon_Manual.md`).
- **L2:** three commits: index schema 2 and the lookup, then the service plumbing, then the extractor schema, its prompt fixture, validation and the change test.
- **L3:** three commits: storage and service with fixture 25, then routes and CLI, then GUI and docs.
- **L4:** four commits: the `ToolContext` literals converted to `..Default::default()`, then the sandbox and tool with events, then the prompt block, then GUI, CLI and docs.
- **L5:** two commits: templates and the ambient grant, then the header and search validity with the subagent ring test.
- **L6:** two commits: the quick wins, then the cache and benches.
- **L7:** two commits: the corpus and its test, then the harness.
- **L8:** the report.

Each substantial commit ends with a line naming what was verified (CLAUDE.md commit style).

---

## 9. Rulings taken in this design

These are adopted on the same basis as 03 §7b and §7c: none reverses an owner decision or widens a component's job. The owner may send any back.

| # | Ruling | Section |
|---|---|---|
| LR1 | Each user row learns into its **own turn's** root, stored on the row as `workspace_root`; a row with none learns into the home partition only; a chunk never mixes roots. This keeps 03 §7a's Extraction row and §3 rule 1 as written. | §1.1 |
| LR2 | The window reads `content` only. Assistant rows with no `request_id` (notices, `post_update`, completion reports) and `/`-turns with their answers stay out; follow-up rows are assistant-written. | §1.2 |
| LR3 | `stated_by: user` needs **every** cited non-context row to be user-authored; a mixed citation is `OtherOutput`. Provenance is checked, not trusted, and the check proves citations, not content. | §2.4 |
| LR4 | The cursor is a sidecar table `learning_cursor`, seeded at `MAX(id)` for existing sessions; `request_id` and `workspace_root` join `conversation_messages` | §1.3 |
| LR5 | At-least-once, with four guards (dedup, already-learned over active notes and candidates, same-key replacement, source suppression), and the store loop and cursor advance in one blocking task | §1.4 |
| LR6 | Stop is a bounded drain (3 s, a constant). The durable guarantee is the boot catch-up. | §1.5 |
| LR7 | Learning starts only after 20 s of quiet and re-checks before each chunk. On a local provider (keyless, or a loopback `base_url`) it yields to a chat turn; on a keyed remote one it starts a chunk only when half the key's RPM and TPM would remain. | §1.7 |
| LR8 | The extraction daily caps read `llm_usage_daily` for the UTC date; a binding cap queues and never drops | §1.8 |
| LR9 | Automatic replacement only over an `extracted_user` target. `user`, `external` and `imported` targets get a suggestion. | §2.5, §3.1 |
| LR10 | A replacement is written in its target's partition | §2.5 |
| LR11 | A learned write with a `fact_key` targets that key's R9 winner in either partition of its chunk. It is a change only when `replaces` says so, when T was never shown to the model, or when a learned T's knowledge differs; a restatement is `unchanged`. | §2.5, §3.4 |
| LR12 | Undo, Keep mine, Deny and S6 **withdraw**: the tombstone carries no links and suppresses no source, so the replaced note comes back and sibling notes stay. Undo also dismisses any candidate. Forget is unchanged. A forgotten candidate's tombstone carries no links. | §3.3 |
| LR13 | `memory_confirm` is offered only on attended `gui`-source main-loop turns, never on connectors (G2, 03 §4), and only for an `extracted_user` suggestion citing a user-authored row | §4.1, §4.2, §4.3 |
| LR14 | A per-call approval is never cached and never auto-approved | §4.2 |
| LR15 | A subagent's notes reach *recently used* through its own memory tools; the empty package path is not edited and is recorded as residue | §6.2 |
| LR16 | The index's FTS layout is measured before it changes | §6.4 |
| LR17 | A learned note whose `fact_key` subject is `user` and that replaces nothing is written to the global partition, whatever its kind | §2.5 |
| LR18 | The index gains `note.supersedes` and `note_source` (schema 2), so learning's per-chunk and per-turn reads never parse a partition | §2.3, §4.5 |
| LR19 | A suggestion is never archived, and never edited by a model tool (S9, S10) | §3.2 |
| LR20 | Only a reachable model's bad output counts as a failed attempt; an unavailable model, a quarantined partition and a store I/O error hold the window with backoff | §1.4 |
| LR21 | A user row over 8 KiB counts as possibly quoted text: an item it supports is `OtherOutput` | §1.2 |

## 10. Owner questions

Each question has a recommendation, and the design is written for it.

> **Answered by the owner, 2026-09-24:** **LQ1 — the same model judges, marked as such** (not the recommendation; §7.4 rewritten). **LQ2 — Deny dismisses the suggestion for good** (recommended). **LQ3 — `memory_read` for every built-in agent by an ambient grant, plugin templates excluded** (recommended). **LQ4 — unlearned turns of a deleted or cleared conversation are dropped** (recommended). Everything below the table is written for these answers; LQ2(b) is not built.

| Id | Question | Recommendation | Alternative and its cost |
|---|---|---|---|
| **LQ1** | Which judge grades the learning evaluation? This machine has only `qwen3.8` (§7.4), the reader's family. | **A cloud judge from another vendor**, keyed through `secret_env` in the scratch `llm.toml`. Independence from the reader is the point, and the corpus is fictional. This restores 04 slice 1's "cloud keys by default". About 0.4M input tokens per 470-question pass, plus about 83 coverage calls per learning run; the report records the actual cost. | A one-time pull of a local model from a **different family** into Ollama: no egress per run and no cost, but a download of tens of GB and about +1 h of local judge time per run |
| **LQ2** | What does **Deny** on the in-chat card do to the suggestion? | **Dismiss it durably**: a withdrawal of the candidate, the same act as **Keep mine** (§3.3). It carries no links and suppresses no source, so whatever else the same message taught stays learned; the cursor is already past those rows, so they are not read again. The card asked "replace X with Y?", and No is an answer. A new optional sandbox hook does it (§4.3). | *Not now*: leave it in the Memory view and do not raise it again on that lane until restart. No hook is needed, but the suggestion comes back in later conversations until it is dismissed in the view. |
| **LQ3** | Should `memory_read` reach every agent by an **ambient grant** in `to_subagent`? That covers the owner's installed templates without a re-seed, and widens the ambient capability set (T15's class). | **Yes, for internal templates only**, together with the shipped-template edits. The owner asked for "every agent template", and the installed `config/agents/` is never re-seeded. `denied_capabilities` still wins. **Plugin-contributed templates are excluded**: a plugin-backed subagent's tool results go back to the plugin's own process (`runner/plugin_agent.rs:241-247`), so an ambient grant would hand every note to third-party code by default. A plugin template that names `memory_read` gets it, as today. | Template edits only: fresh installs only, and the owner hand-edits three files. Or an ambient grant for plugin templates too, which exposes the owner's notes to every installed plugin agent without a prompt. |
| **LQ4** | What happens to the unlearned turns of a conversation the user **deletes or clears**? | **Dropped.** The delete is the user's say, learning never delays a delete, and an in-flight flush that finds its session gone writes nothing (§1.5). MD-10's "nothing is skipped" then reads "nothing the user kept". | Extract first: the delete waits on a model call, tens of seconds on a local model, or races it |

**Raised while building (L1–L10c), PENDING — none of LQ5–LQ10 is adopted.** Each is a behaviour an implementer took where this design was silent or where its first reading proved wrong; the first column is what ships, and the shipped code is the "no change" answer. The recommendation is only that: applied nowhere, decided by nobody but the owner. The same rows are in the owner's decision log (rev 3.22).

| Id | What ships (the "no change" answer) | Alternative | Recommendation | Where built |
|---|---|---|---|---|
| **LQ5** (L1, rev 3.2) | Learning **drops** a chunk whose project directory no longer exists (`skipped_root_gone`; since residue R7 the check runs again just before storing, so a directory deleted while the model answered is not re-created either): it is not learned into home and not held. Risk: an unmounted volume reads as deleted, and its turns are dropped. | Hold the chunk with backoff until the directory is back, or learn it into the home partition (breaks LR1: project facts filed as global) | **Keep.** | `memory/learning/queue.rs` (the pre-call check), `memory/learning/extractor.rs` (`root_gone`, the in-store check; also the episode writer, `episode.rs`); `5934a4ab`, `026d5de0` |
| **LQ6** (L4, rev 3.7 deviation 2) | A **Deny or timeout** on a suggestion's card hides every later suggestion for the **same target note** on that lane — every app and CLI conversation of the owner — until the daemon restarts, because the raised set records the target as well as the suggestion. | Narrow it to the same candidate, so a restatement (a new suggestion for the same note) is raised again | **Keep.** | `MemoryService::mark_raised` / `raised_on` (`memory/service/in_chat.rs:453`, `:464`), set by `memory_confirm`'s preview hook; `d27c034a` |
| **LQ7** (L7, rev 3.11) | The L-S system seeds the four gold notes that record their own replacement (`user.editor`, `user.laptop`, `user.commute`, `moss.output_dir`) **present-tense, with no `valid_to`**, as the user saved them then. Evaluation only; no product behaviour. | Seed the gold text as written (§7.2) and expect suggestions for the other four pairs only | **Keep.** | `LS_SEEDED_AS_SAVED_THEN` (`crates/openalpaca_core/tests/memory_eval.rs:308`); `b5c942e7` |
| **LQ8** (L10b, rev 3.15) | The per-turn recall's index lines carry the **full UUID**, not §13.2's `<id8>`, because `memory_read` takes full UUIDs only. | `memory_read` accepts a unique prefix of at least 8 hex characters (widens L10a's contract), and the lines go back to `<id8>` | **Keep.** | `memory/service/recall.rs` (the index block, `INDEX_BLOCK_HEADER`), `memory_read` in `tools/builtins/memory_tools.rs`; `c812dfbb` |
| **LQ9** (L10c, revs 3.17, 3.18) | Automatic recall leaves out **conversation summaries only** — `kind = episode` **and** origin `extracted_other` **and** not a candidate — not every `episode`; a **pinned** summary is still delivered; and **forgetting a summary suppresses no source** (its tombstone is a withdrawal's), so what the conversation taught stays learned. | The literal "every episode" for the recall filter, and an ordinary forget that suppresses the cited messages | **Keep.** | `memory_files::is_conversation_summary` (`memory_files/mod.rs:130`), the index's `SearchMode::Automatic` (`memory_index/search.rs`), the forget mode (`memory/service/mutations.rs`); `df835d96`, `5da63a2e` |
| **LQ10** (L10c, revs 3.18, 3.19, last re-review) | A model tool's edit or archive of a summary **keeps origin `extracted_other`**, so it stays a summary; a model-added `fact_key` on a summary is **ignored by learning** (no target, not in `<known_keys>`) but the note still **groups by key**: a "conflict" tag in the GUI (`MemoryRow.tsx`, and the detail's warning in `NoteTab.tsx`) and a `same_fact_key` pair in `memory doctor`, both only when an active note with the same key sits in the same partition. | Refuse a non-user edit that sets a `fact_key` on a summary (`authority_conflict`, as S10 refuses a tool's edit of a suggestion); or leave summaries out of same-key grouping (changes frozen 03 §3 rule 3 / §7c R9) | **The narrow refusal** — a recommendation only, **not applied**. | `Partition::update` (`memory_files/partition.rs`), `MemoryIndex::fact_keys` / `fact_key_winner` (`memory_index/mod.rs:804`, `:925`), `memory_files/authority.rs`; `7a9b1de7`, `13ba21ac` |

**Raised by the L8 evaluation, TAKEN by the owner on 2026-09-26 (rev 3.33).** Each rests on the L8 diagnosis and capture in the session scratchpad (`memeval/l8/diagnosis/SUMMARY.json` and its notes, each with its sceptics' corrections; `memeval/l8/capture/`; `memeval/l8/bench/scale/analysis.md` with its sceptic review under `bench/scale/sceptic/`). They belong in the owner's decision log and in `tasks/api-fix-plan.md` §0 too, which this revision does not edit.

| Id | Question | Decision | Evidence | Where built |
|---|---|---|---|---|
| **LQ11** | A chat model's `memory_store` naming a note the user saved in `supersedes` was `authority_conflict` (§3.1 row 5), after which the model stored the new value as a second, contradicting active note, and no suggestion was raised (L-S: 4 of 8 suggestions missing). | **Reversed: such a replacement is a suggestion** the user confirms, through learning's path (S1–S4); nothing active changes until then. The lead's reading, implemented: a store under such a note's `fact_key`, and an edit of such a note, are the same replacement; archive of a user note and forget are unchanged; G2 holds. §3.1 row 5 amended in place. | `diagnosis/ls-suggestions.md`, `ls-suggestions-sceptic1.md` | `9f1bb2f3`, `abdd9645`, `79f00f49` |
| **LQ12** | The learning metrics counted the reader's own `memory_store` notes as learned, mixing two systems (L live against the extraction-only replays). | **Report the metrics by surface** (extraction / model tool / whole store). | `diagnosis/learning-gap.md`, `learning-gap-sceptic1.md`, `learning-gap.sceptic-2.md`, `completeness.md` | done in the L8 fix slice's harness work: `bf094ca8` (H3, rev 3.26) |
| **LQ13** | About one extraction answer in five was unusable; the capture found nine of its eleven a bare array of items, one followed by `</think>`, one after prose. | **Tolerant unwrap**: `<think>` blocks and prose around the answer stripped, the one JSON value it carries taken (the caller's shape preferred), a bare items array read as the items; every item still passes every rule; a truncated answer is never rescued, and a cut one is still reported as cut. §2.4 rule 1 amended in place. | `diagnosis/rerun-badoutput.md` and its two sceptics; `capture/unusable.json`, `capture/report-replay-capture.json`, `capture/full-replay-capture.log` | `6120812b` |
| **LQ14** | The extractor was told to reuse known keys, and the reader's coarse or value-bearing keys (`editor.preference`, `routine.friday.weekly-review`, `user.machine`) led it to replace or drop unrelated facts. | **Keys only the chat model coined are not offered** to the extractor (known keys, key targets, the note's line); the notes stay searchable, recallable and listed; a key learning or the user also holds is offered. No `memory_store` key validation. §2.3 step 4 amended in place. | `diagnosis/learning-gap.md` (owner decision 4), its two sceptics | `0fe524d8` |
| **LQ15** | Which store size does the memory service's cost target? | **The design point is about 30K notes**; 100K is a later target. | `bench/scale/analysis.md` and its sceptic review | — (a target, no code) |
| **LQ16** | Drop ruling R3's stat scan before every read (option O4), or keep it? | **Keep R3, and make the scan cheaper**: a bulk directory stat where the platform has one (`getattrlistbulk` on macOS; measured 221–233 ms → 99–108 ms per 100,000-file scan), `readdir` + `lstat` elsewhere, the same entries and fields bit for bit. | `bench/scale/analysis.md` (its sceptic section), `bench/scale/sceptic/scanbench.c` | `0177a41f` |
| **LQ17** | The index's FTS layout at 100K. | **An index-layout revision, in a separate slice**, which follows this one. | `bench/scale/analysis.md`, `bench/scale/sceptic/` (the `ftsq` measurements) | — (next slice) |

**Noted, not asked.**
- L4 makes T22's unscoped `GET /v1/chat/confirmations` carry note titles and excerpts in a memory-confirm preview (§4.4). T22 stays pending as it is.
- The stop drain stays. A review proposed replacing it with the boot catch-up alone, since the drain rarely finishes on a local model. MD-10 names "before the daemon stops", though, so removing the drain is the owner's call, not this design's. It costs one constant and a few lines once the queue exists.
- Restoring a note that a *confirmed* suggestion retired stays unbuilt, as at HEAD (§0.3).

## 11. What this changes in 03's frozen contracts (§7a) and rules

These are design revisions. The plan was for each slice to fold its own rows into 03; only L9's was (03 rev 6), and **03 rev 7 folds every other row below** (rev 3.22), citing the 09 revision that introduced each. The rows from *§7a route table, undo* down were added at rev 3.22 so that this table is the complete list of what Learning v2 changed in 03; two of them are the lead's rulings of 2026-09-25 on frozen contracts (03 §7a).

| Where in 03 | Change | Slice |
|---|---|---|
| §4 *Initial limits* | "Extracted notes per chat turn" becomes "per extraction call, `extract_max_items_per_call` (12)" | L1 |
| §6 *Promotion and correction*, automatic extraction | The cadence (§1), the database window reading `content` only, each row learning into its own turn's root (stored as `workspace_root`, which keeps §7a's Extraction row true), the byte budget, the durable daily caps, and what holds a window (an unavailable model, a quarantined partition) | L1 |
| §7a `MemoryAccessContext`, Extraction row | Adds: "reads through `learning_view` only: metadata and bounded excerpts, not tool-reachable, never recorded". Capabilities stay write-only. | L2 |
| §3 rule 2 | Adds: "a learned note whose `fact_key` subject is `user` and that replaces nothing is global" (LR17) | L2 |
| §4 origins; §6 derived supersession | A candidate may name a `user` target (S1). A user-words **suggestion** is a candidate of origin `extracted_user`. A candidate's tombstone carries no links (S3). A suggestion is never archived, and a model tool never edits one (S9, S10). | L3 |
| §6 *Promotion and correction*, recently learned and undo | Recently learned lists recallable notes only. Undo **withdraws**: no links, no suppressed source, and the replaced note comes back (LR12). Undo also accepts a candidate of any origin, which is how a suggestion is dismissed. Forget is unchanged. | L3 |
| §6 *Forget contract* | Adds: "a withdrawal (undo, dismissal) is not a forget: it suppresses no source" | L3 |
| §7a list item; `memory_list`; `recently-learned` | `supersedes: [uuid]` | L3 |
| §7a route table | `GET /v1/memories/suggestions`; confirm's `target_revision` (required for a suggestion); undo accepting a candidate; the confirm result's `superseded` and `dismissed` | L3 |
| §7a `memory_update` refusals | `invalid_input` for an archive of a suggestion (every surface); `authority_conflict` for a `ModelTool` edit of one | L3 |
| §4 user-directed surface; §7 tool table; §7a tool contracts | `memory_confirm` (§4.1), limited to `extracted_user` suggestions citing a user-authored row, and the one-sentence addition in §4.2 | L4 |
| §7 *Tool annotations* | `approval_is_per_call`; the preview hook; `memory_update`'s confirm refused before the card | L4 |
| §7 *Capabilities* (line 359) | "No agent template changes" becomes "all nine templates name `memory_read`, and `to_subagent` grants it ambiently to internal templates unless denied; a plugin-contributed template gets it only by naming it" | L5 |
| §5 step 8, the header; §7a `memory_search` result | `valid_from`, `valid_to` and `timing=upcoming` in the header; `valid_from` and `valid_to` on search results | L5 |
| §7a route table, undo | **Lead ruling, 2026-09-25 (rev 3.5):** `POST /v1/memories/{id}/undo` answers `{id, status: "withdrawn", restored: [uuid]}`, not `{id, status: "forgotten"}` with `retained` — a withdrawal suppresses nothing, so `retained` would always read false; `memory_changed` still says `change: "forgotten"` | L3 |
| §7a route table, confirm | A suggestion whose target is no longer effectively active is refused `invalid_input` (400), naming undo; a target that is gone is `404` (rev 3.6, `5774e8cc`) | L3 |
| §7a *Events*; `GET /v1/chat/confirmations` | `ToolConfirmationRequested` (both enums and SSE) and the pending-confirmations read carry `preview` (daemon-written, `null` on the ordinary card) and `per_call`; live only, the `event_log` row keeps ids (rev 3.7) | L4 |
| §7 tool table; §7a `memory_search`, `memory_read`, new `memory_timeline`; route table; CLI syntax | `memory_search`: `detail` (`index` default, `snippets`), `date_from`/`date_to`, `order_by`, `limit` 1–50 default 10, the index row. `memory_read`: `ids` (1–10), per-id `not_found`/`revision_conflict`, one shared 32 KiB. `memory_timeline` and `GET /v1/memories/timeline`. The route's `detail` defaults to `snippets`. `openalpaca memory search --since/--until/--order` and `memory timeline` (rev 3.14) | L10a |
| §5 step 8; §7a `memory_used` | `[orchestrator.memory] recall_mode`: `index` (default) = pinned notes, the top 2 as excerpts and up to 8 index lines; `excerpts` = the delivery before; a skill that cannot call `memory_read` is recalled in `excerpts`; `memory_used.reason` gains `index` (revs 3.15, 3.16) | L10b |
| §4 origins; §5 automatic filter; §6 forget contract; §7a writers; §3 rule 3 | A conversation summary (`episode`, origin `extracted_other`, not a candidate) is never recalled automatically (a pinned one is); forgetting one suppresses no source; a conversation delete, clear or purge forgets its summaries; a non-user edit or archive keeps its origin; it holds no key for learning; `ExtractionProvenance::Summary` (revs 3.17–3.19) | L10c |
| §6 write protocol, step 4; §5 freshness | **Lead ruling, 2026-09-25 (residue R6, `7d5174e9`):** an index failure after the durable step 3 no longer fails the write — it is logged (one WARN, partition key and error), the partition's cached fingerprints are dropped so the next reconciliation re-indexes it whole, and the next search's stat scan re-indexes first | residue |

## 12. Chat memory commands (MD-13, slice L9)

> **Owner, 2026-09-24 (MD-13):** add direct chat commands `/remember`, `/forget` and `/memories`, handled by the daemon with **no model involved**, like `/status` and `/tasks`. Because the human typed them, `/remember` saves with origin **`user`** — the in-chat equivalent of the Memory view. Offered only on attended app/CLI turns. No memory *skill* (SKILL.md) is added.

**Why this is G2-safe.** G2 forbids a *model* tool call from minting origin `user`. These commands never reach a model: the deterministic tier (`orchestrator/intent/mod.rs`, parsed before skills and before the main loop; `orchestrator/handlers.rs`) executes them and answers with a fixed template. The text that becomes the note is exactly what the human typed on an attended turn. The user-directed surfaces of 03 §4 therefore gain one member — *a deterministic memory command typed on an attended app or CLI chat turn* — and 03 §7a's `MemoryAccessContext` table gains that row with `surface: UserSurface`.

### 12.1 The commands

| Command | Behaviour | Reply (deterministic) |
|---|---|---|
| `/remember [--kind <k>] [--global \| --project] <text>` | Stores a note through `MemoryService` with `surface: UserSurface`, origin `user`. `kind` defaults to `fact` (the six kinds of §4 are accepted); the scope defaults by kind (03 §3 rule 2) against the request's marker-found project root; `--global` / `--project` override (`--project` with no project root is `invalid_input`, §7b P1). Title: the first line, cut at 80 characters on a char boundary; body: the whole text. Exact dedup answers the existing id (§7b P8). | `Saved as a note you wrote: “<title>” (<id8>, <scope>). It is used from your next message.` — or the refusal's own sentence. |
| `/forget <id-or-prefix>` | Resolves a **unique** id prefix (at least 8 hex characters) among the notes this request can read; fetches the current revision; forgets it (tombstone-first, §6). An ambiguous prefix lists the matches and forgets nothing. | `Forgot “<title>” (<id8>).` followed by the service's retained-copies sentence. |
| `/forget <text>` (not an id-shaped token) | Runs `search` (limit 5, active notes) and **forgets nothing**. | The matches as `<id8>  <title>  [<kind>/<scope>]` lines, then `Send /forget <id> to forget one.` — or `Nothing matched “<text>”.` |
| `/memories` | The 10 most recently updated active notes this request can read, and the number of pending suggestions (L3). | One line per note as above; `N suggestions are waiting in the Memory view.` when N > 0. |
| `/memories <query>` | `search` (limit 8, active notes). | One line per hit with its match reason; `Nothing matched “<query>”.` |

Output is bounded (at most 10 lines of notes, each line at most 160 characters). No note body is printed beyond the title; `/memories` is a pointer, not a reader (`memory_read` and the Memory view read).

### 12.2 Where they run, and where they refuse

- **Attended app/CLI turns only** (`unattended == false`, lane source `gui` or `cli`). A connector turn (Telegram, Discord, iMessage), an unattended app or CLI turn (a piped `openalpaca chat`), a scheduled skill, a follow-up and `/v1/command process` get: ``Memory commands work only in the OpenAlpaca app's chat, or in `openalpaca chat` run at a terminal — not with its input or output piped.`` — deterministic, nothing written (rev 3.22, residue R5: the first wording, "…work in the OpenAlpaca app and the CLI.", contradicted itself to a piped CLI; `orchestrator/memory_commands.rs`, `NOT_HERE`). (Same boundary as `memory_confirm`, §4.3; the iMessage non-owner defect is why connectors are excluded.)
- **No owner** (`Principal::System`) or **no memory service**: `Memory is not available here.`
- **Attachments** on a memory-command turn are skipped with a reason (`skip_turn_attachments`, as the other deterministic arms do).
- **Learning** never reads these rows: the transcript window already leaves out user rows starting with `/` together with their answer (§1.2).

### 12.3 Parsing and reserved names

- `IntentParser` gains `Intent::MemoryCommand { verb: Remember | Forget | Memories, args: String }`, parsed before skill matching — on the production path (`parse_with_skills_and_router`) before task control too, which the three words cannot collide with (rev 3.22: this line said "after task control"); the command word is case-insensitive and must be followed by whitespace or end of input (`/remembering` is not a command). The args are kept **verbatim** (not lower-cased).
- `/remember`, `/forget` and `/memories` become **reserved slash names**: a skill whose name is one of them is not reachable by slash (the skill loader logs one WARN naming it at load), and `create-skill` refuses those names.
- `intent_type()` gains `"memory_command"` for telemetry.

### 12.4 Plan row L9

| Slice | What lands | Depends on | Size |
|---|---|---|---|
| **L9** | `Intent::MemoryCommand` and its parser; `orchestrator/memory_commands.rs` (the three handlers over `MemoryService` with a `UserSurface` context built from the attended turn); the refusals of §12.2; reserved names in the skill loader and `create-skill`; `memory_changed` published by the service as for any write; docs | L3 (suggestion count), L5 (none) | S–M |

**Files:** `crates/openalpaca_core/src/orchestrator/intent/{mod.rs,tests.rs}`, `orchestrator/handlers.rs`, `orchestrator/memory_commands.rs` (new) + tests, `memory/service/context.rs` (the new constructor), the skill loader and `config/skills/create-skill/`, `docs/CLI_Manual.md` (chat commands), `docs/GUI_Manual.md`, `docs/Daemon_Manual.md` (the deterministic tier), 03 §4/§7a (the new user-directed surface and table row).

**Evidence (tests first):**
- the parse table: each verb, case, verbatim args, `/remembering` not a command, a skill named `remember` not reached by slash;
- `/remember …` on an attended GUI turn writes one note with origin `user` and `surface: UserSurface`, **with zero model calls** (a scripted provider that fails the test if called);
- `--kind`, `--global`, `--project`, and `--project` with no root refused `invalid_input`;
- the same command on a connector turn, an unattended turn and a `System`-principal turn writes nothing and answers the refusal sentence;
- `/forget <id8>` forgets exactly that note (tombstone present, file gone); `/forget <ambiguous prefix>` and `/forget <text>` forget nothing and list matches;
- `/memories` and `/memories <query>` are bounded and print no body;
- one `memory_changed` per write; the rows are not in the next learning window.

## 13. Layered retrieval, index-first recall and conversation summaries (MD-14…MD-16, slice L10)

> **Owner, 2026-09-24:** adopt three ideas from claude-mem's `mem-search` / `how-it-works` skills (`github.com/thedotmack/claude-mem`, `plugin/skills/mem-search/SKILL.md`, `plugin/skills/how-it-works/SKILL.md`, read 2026-09-24): **MD-14** layered memory tools; **MD-15** index-first recall each turn, measured against excerpt recall; **MD-16** conversation summaries as `episode` notes, findable but never injected automatically. The reference's rule is "filter first, fetch later": a search returns a compact index (about 50–100 tokens a result), a timeline shows what surrounds an entry in time, and full details are fetched only for chosen ids, in one batch — the reference reports about 10× fewer tokens.

### 13.1 Layered memory tools (MD-14, L10a)

All three stay under the `memory_read` capability (no new grant).

| Tool | Change |
|---|---|
| `memory_search` | New `detail: "index" \| "snippets"`, **default `"index"`**. An index row is `{id, date, kind, scope, status, title, reason, size}` — `date` is `observed_at` when present, else `created_at`; `reason` is the match reason; `size` the note's approximate token count (bytes ÷ 4) so the model can budget a read. `"snippets"` is today's output. New `date_from`, `date_to` (inclusive, on `date`) and `order_by: "relevance" \| "newest" \| "oldest"` (default `relevance`). `limit` default 10, max 50. The filters of §5/§7b are unchanged. |
| `memory_read` | Takes `ids: [..]` (1–10) instead of one `id`; one call returns every note, each with its revision and provenance, inside the unchanged 32 KiB total (split evenly; a cut note says so). A single id is `ids: [id]` — the app is unreleased, so the old argument breaks cleanly (03 §7). Each id is re-checked for access as today. |
| `memory_timeline` (new) | Input: exactly one of `anchor` (a note id), `query` (the first search hit becomes the anchor) or `date` (an instant); `before` and `after` (default 5, max 20). Returns index rows (the `memory_search` index shape) in time order around the anchor, **including** superseded, archived and ended-window notes marked by `status`, and `episode` notes (§13.3), so "what did I decide before the June move" and "what did we discuss last week" have one tool. Candidates and suggestions are excluded. |

The three tool descriptions teach the layered workflow in two sentences each ("Search returns an index; pick ids; read only those — several at once"). The main loop's memory guidance in the system prompt says the same in one line. API and CLI parity: `GET /v1/memories/search` accepts `detail`, `date_from`, `date_to`, `order_by`; `GET /v1/memories/timeline` is new; `openalpaca memory search --since --until --order` and `openalpaca memory timeline <id|--query q|--date d>` are new. The GUI keeps its current calls.

### 13.2 Index-first recall each turn (MD-15, L10b)

A new key `[orchestrator.memory] recall_mode = "index" | "excerpts"`, **default `"index"`** (owner-adopted; §13.4 measures it against `"excerpts"`, and a regression is reported to the owner, not silently reverted).

- `"excerpts"` is today's §5 delivery (up to 8 chunks, 8 KiB).
- `"index"` delivers, inside the same 8 KiB: the pinned notes (unchanged, 2 KiB sub-budget); the **top 2** recalled notes as excerpts; then an **index block** of up to 8 further relevant notes, one line each — `<id8> <date> <kind> “<title>” — <reason>` — under the line `More notes that may be relevant (read with memory_read):`. All of it stays quoted reference data (03 §5 step 8).
- `memory_used` gains `reason: "index"` for a listed note; the recently-used ring records it as listed, not delivered.

### 13.3 Conversation summaries as `episode` notes (MD-16, L10c)

- **When:** at the end of a conversation's quiet period of **at least 30 minutes**, on a conversation switch, and in the shutdown drain — the MD-10 triggers minus the 120-second idle one, which is too frequent for a summary — and only when at least **4 user turns** have accumulated since the session's last episode. The window, byte budget, cost gates and yielding to chat of §1 apply; one call per episode.
- **What:** one note, `kind: episode`, title `Conversation on <YYYY-MM-DD>: <topic>`, body: what was discussed, what was decided, what was left open, in at most 1 KiB of prose; `observed_at` = the last turn's time; `sources` = the session's message rows in the window; no `fact_key`, no aliases required, no `supersedes`.
- **Authority and visibility:** origin `extracted_other` (the model's own wording, lowest rank, so it can supersede nothing). Unlike a candidate, an episode is `active` so that `memory_search`, `memory_timeline`, `memory_list` and the Memory view find it — but **automatic recall excludes `kind = episode`** (per-turn recall and the index block alike), so it is never injected on its own. This is a stated exception to MD-2's "assistant-derived stays a candidate", justified by the owner's choice: an episode is a record of a conversation, not a claim about the user, and it never enters a prompt unless the model or the user asks for it.
- **Forgetting:** `/forget`, the Memory view and `memory_forget` work as for any note. Deleting or clearing a conversation (LQ4) also **forgets the episodes whose sources all lie in that session**.
- **Cost:** counted under the same durable daily gates as extraction (§1.8), with its own per-day count cap `episode_max_per_day` (default 20).

### 13.4 Plan rows L10a–L10c

| Slice | What lands | Depends on | Size |
|---|---|---|---|
| **L10a** | `memory_search` index/snippets, date filters, `order_by`; batch `memory_read`; `memory_timeline`; the descriptions and the one-line prompt guidance; route and CLI parity | L5 | M |
| **L10b** | `recall_mode` with the index block; `memory_used` reason `index`; the config key with its registry row, `daemon_config_cli` row and `config/daemon.toml` line | L10a | S |
| **L10c** | Episode writing on the three triggers; the recall exclusion; the conversation-delete forget; `episode_max_per_day` | L1, L10a | M |

**Evidence (tests first):**
- L10a: an index row carries no body and every field above; `date_from`/`date_to` bound on `observed_at` else `created_at`; `order_by` newest/oldest; a 10-id `memory_read` returns all within 32 KiB and refuses an 11th id; a foreign id in the batch is refused without leaking the others' order; `memory_timeline` by anchor, by query and by date returns neighbours in time order with superseded/ended notes marked and candidates absent; the route and CLI mirror the tool.
- L10b: `recall_mode = "excerpts"` reproduces today's delivery byte for byte (a golden test); `"index"` delivers pinned + 2 excerpts + at most 8 index lines inside 8 KiB; an index note appears in `memory_used` with reason `index`.
- L10c: no episode before 4 user turns; one episode after a 30-minute quiet period, a switch, or the shutdown drain (the harness's deterministic flush drives all three); an episode is returned by search and timeline and **never** by per-turn recall; deleting the conversation forgets it; the daily cap holds.
- L8 gains: the D run in both recall modes (`D-excerpts`, `D-index`), reporting accuracy, input tokens per turn and tool rounds; and a small **episode question split** (`scripts/eval/memory/questions-episodes.json`, about 20 "what did we discuss / decide on …" questions over the learning corpus), answered after a learning run.

## Revision log

The log is not in date order and is not reordered. The 3.x entries are the implementers' and continue rev 3, the approval; the lead's **rev 4** and **rev 5** (MD-13…MD-16, `dac8221e`, `2fbe8e06`) were written between rev 3.4 and rev 3.5 and so predate every entry from rev 3.5 on despite their numbers; and rev 1 sits above rev 2.

- **rev 3.33 (2026-09-27), the L8 product slice: the owner's decisions LQ11, LQ13, LQ14 and LQ16 of 2026-09-26, and the scale slice's open items (`6120812b..79f00f49` and the documentation commit that records this entry).** Each behavioural change tests first — red on the unchanged code, then green — and each pin shown to bite under a negative control. §10 gains LQ11–LQ17 as taken; §2.3 step 4, §2.4 rule 1, §3.1 row 5 and §3.2 S2 are amended in place, each pointing here; 03's contracts are rev 7.4.
  - **LQ11** (`9f1bb2f3`, `abdd9645`, `79f00f49`): a chat model's replacement of a note the user saved, edited by hand or imported is a **suggestion**, where §3.1 row 5 made it `authority_conflict` and the model then stored a second, contradicting active note. On the `ModelTool` surface every `supersedes` goes through `ReplacePolicy::SuggestUnlessLearned` (S1–S4), so the file layer, under the partition's lock, writes a candidate naming a saved target and lets a replacement of a learned note stand. **The lead's reading, as built:** (1) a store with no `supersedes` whose `fact_key`'s R9 winner — the chunk's-project-first lookup of LR11, over the partitions the store may go to — is a saved note names that note, where it used to land as a same-key loser the user never saw; (2) a `memory_update` edit of a saved, effectively active note that changes its title, body, aliases, tags, key or validity writes a suggestion carrying the edited note (`UpdateOutput.suggested_for`), the note untouched — an edit that changes nothing, or only `review_after` (which a suggestion cannot carry), is answered as any edit (G2 refuses it for a `user` note); (3) archive of a `user` note stays `authority_conflict`, and forget is unchanged — it was never refused, only confirmed on the card, so "stays refused" reads "stays as it is". G2 holds: the suggestion is `extracted_user`, retires nothing, and only a user-directed confirm makes it active. Choices the spec left to the implementer, recorded:
    - **The suggestion cites the turn's own row** the user typed (`ConversationRepository::user_row_of_request`, by the turn's request id; a row that is not user-authored in §1.2's sense — an internal turn, a `/slash` line, a row over 8 KiB — is not cited). Without it §4.2 rule 5 would refuse the suggestion on the in-chat card, which LQ11 names as one way to confirm it; with it `<memory_suggestions>` lists it, and learning's rule 13 keeps the extractor from raising a second suggestion for the same note (the sceptic's point, `ls-suggestions-sceptic1.md`). The note-level `observed_at` stays empty — the extractor's alone, and LQ14's datum.
    - **Ruling LR10 on the tool path:** with no `scope`, the suggestion (and any replacement the model names) is stored in its target's partition, as learning's is — a global saved note changed in a project chat was `invalid_input` under R4. Every other surface keeps R4 (`79f00f49` narrowed a first cut that changed it for all).
    - **S2's condition is "the target is saved"** (`Origin::is_saved`: `user`, `external`, `imported`), where it read "not `extracted_user`" — the same set for learning, whose targets are never `extracted_other`, and it keeps a chat model's replacement of a conversation summary what it was.
    - **Wording.** The tool results say the change is saved as a suggestion, not applied, how the user confirms it (Memory view, `openalpaca memory confirm <id>`), that it may be raised with `memory_confirm` on a later attended turn, and not to store it again; `memory_store`'s and `memory_update`'s descriptions state the rule. The Memory view renders it as an ordinary suggestion but says who made it — "Suggested by Alpaca on <date>" on the card, "Suggested by Alpaca — would replace “X”" on the detail — and `openalpaca memory suggestions` "suggested by the assistant, <date>", told apart by LQ14's datum (`extracted_user`, no `observed_at`); learning's read as before. The in-chat card's "from what you said on <date>" is unchanged: it cites the row the user typed.
  - **LQ13** (`6120812b`): `task_extraction::parse_json_answer` strips a leading `<think>…</think>` block and a trailing `</think>`, reads the answer whole or fenced as before, and otherwise takes the complete top-level JSON values in the prose — the last of the caller's shape, else the last. A value opened and never closed fails the whole answer; a bracketed stretch that is not JSON is skipped to its matching bracket, so nothing is dug out of it. Chat learning reads a non-empty bare array of item objects as the items list; the episode summary and task-output extraction pass their own shapes. An answer cut at `max_tokens` is held to the forms read before (`parse_cut_items`, `parse_cut_summary`, task extraction's strict read), so the fix slice's P1 stands. The capture's 11 answers are fixtures (`learning/tests/unusable_answers.json`): the capture kept 400 characters of each end, so each fixture is that head and tail joined by a written bridge closing the head's item and opening the tail's; with negative cases (the capture's own truncated head, malformed JSON, two objects of the wrong shape, prose alone, empty and non-object arrays).
  - **LQ14** (`0fe524d8`): `known_keys` and the key-target lookup leave out a key held only by notes the chat model's `memory_store` wrote; the note stays searchable, recallable and listed, with no key on its line; a key a learned or a saved note also holds is offered, with its R9 winner as the target. **The datum is origin `extracted_user` with no `observed_at`**, not `sources: []` as the diagnosis put it: the service refuses `observed_at` on every surface but `Extraction`, a patch cannot carry it, and every note the extractor writes from the user's words has one (its newest cited row's `created_at`, SQLite's `datetime('now')`); `memory_store` accepts `sources`, so a tool note can cite a row (2 of the reader's 92 in L did). Residue: a note learned before L2 has no `observed_at` but no `fact_key` either; a chat model's edit that sets a new key on a learned note leaves it counted as learning's. No snapshot field and no index schema change (`LEARNING_KEY_HOLDER`, a condition on existing columns).
  - **LQ16** (`0177a41f`): R3 stays; the stat scan lists `notes/` with one `getattrlistbulk(2)` pass on macOS (`memory_files::dirstat`), `readdir` + `lstat` elsewhere. A test runs both over one directory of every kind of entry and compares each entry's `ScanEntry` and `StatKey` bit for bit (macOS only). Measured, release, 100,000 files: 221–233 ms → 99–108 ms per scan.
  - **P5** (`44644158`, the scale slice's round-3 test review): a note renamed by hand, a same-size edit in place and a same-bytes rewrite with a moved mtime are each caught by the incremental path as a rebuild catches them, after a restart too; a fingerprint blind to `relative_path` or to the stat fails it.
  - **P6** (`ec92c2e1`, the round-3 safety review's F1): a rebuild opens, reads and indexes each partition under its writer slot, so a write lands before the open or after the rows — never between, where it was indexed with the stale derived facts of the open's snapshot. A tick records its scan only when its pass ran, and a completed rebuild forgets the ticks' last scans. Tests interleave a replacement and a forget with a held rebuild and a tick.
  - **P7, P8** (`d4522406`, the round-3 consistency review's nit and the scale slice's concerns): only background learning's lookups wait for a rebuild (`touch`, `read_index`); `touch`'s doc now says so. Recall and search never wait, not even for a rebuild that starts under their stat scans (`serving_or_files`, `touch_serving`); the pending suggestions list nothing at once. Per call site: `memory_timeline` — the index's date order, with no file answer — refuses at once with `index_rebuilding` (route: `503 MEMORY_INDEX_REBUILDING`), a refusal the model relays; the suggestions list (the Memory view, the CLI, `/memories` on a chat turn) and a deleted conversation's summaries are read from the partitions' files, complete (`read_index_or`).
- **rev 3.32 (2026-09-26), the L8 scale slice's repair round 2 (`e3df69b2..fa4bca8e` and the documentation commit that records this entry).** Three reviews of rev 3.31 (tests, safety, consistency) raised nine findings — 1 minor, 8 nits. The lead passed on five (1 minor, 4 nits), and each is fixed; none is rejected. Three are tests and two are wording. Not in this round: the tests lens's daemon-wiring pin, carried from round 1 and still optional; and the consistency lens's N2 (S5's bloat-bound measurements were taken before the idle merge worked — a scratchpad approximation puts a quarter of the notes edited six times at 1.18× with it, a whole rewrite unchanged at 1.74× — and `MERGE_PAGES`'s doc over-generalises why a positive merge found no work) and N3 (CLAUDE.md's Code Navigation row does not list `memory_index/scan.rs`). The safety lens's one finding, that round 1's F5 was neither fixed nor recorded, is answered by rev 3.31 now naming F5 among what that round left out. **No behaviour changes, and no frozen contract (03 §7a) or ruling (03 §7b/§7c);** 03 is unchanged. Each new or changed test was shown to fail under a negative control.
  - **S1's seeding of a row it cannot read** (`e3df69b2`, tests): only a damaged or foreign cache has such a row, so the two defensive paths had no test. `a_row_the_seeding_cannot_read_is_removed_or_written_again` plants a stray row under `not-a-uuid`, and `origin`, `effective_status` and `superseded_by` values that do not parse on three other rows. A restarted service's tick builds three rows and leaves every note column, chunk, alias and citation as it was before the damage. The status and `superseded_by` rows pin the forced rewrite, because the values the seeding falls back to (`active`, no successor) are those notes' own. Negative controls: without the stray removal the stray row stays; without the empty revision only the origin row is written.
  - **S6's daemon-open test** (`433d69c4`, tests): it could not tell a file scan from an inline index of the fresh cache. While a rebuild is owed but not running, `touch` indexes the partition and the meta still says `rebuilding`. And its search during the held rebuild ran on the test's own thread, so a search that waited hung the test. It now asserts that the index holds no note after the search and recall before the rebuild. The search and the suggestions during the rebuild answer on their own thread under a 10 s `recv_timeout`. Negative controls: search always touching, recall always touching, search touching while the rebuild runs, and `pending_suggestions` waiting for the rebuild each fail, the last two on the timeout.
  - **S7's reply-path wrappers** (`66e8260c`, tests): only `REPLY_MEMORY_DEADLINE`'s value was pinned, and the stall tests pass their own 100 ms. `the_reply_path_waits_on_memory_for_the_reply_memory_deadline` holds the reads and calls `ContextSource::resolve` and `memory_suggestions_block` as a turn does. Each returns empty after at least the deadline and less than 2 s past it, under the outer 5 s bound. Negative controls: either wrapper at an hour fails on the outer bound, and `resolve` at 100 ms fails the lower bound.
  - **`pending_suggestions` can wait** (`fa4bca8e`, consistency): its doc and `serving_generation`'s said the turn goes without the block "rather than wait", and rev 3.31's `f757df76` message says it never waits. The rebuild check is made once, at the start. A rebuild that starts after the check and before a partition's `touch` makes that `touch` wait for the whole rebuild (about 33 s at 100K notes). S7's 1.5 s bound cuts the turn's wait short, but the blocking worker stays held until the rebuild ends, and the read then lists nothing. Both comments now say so.
  - **Rev 3.31's count** (this commit, consistency, minor): it said the reviews raised 16 findings. They raised 22, and the lead passed on 16. It is corrected in place, and it now names the six the round left out.
- **rev 3.31 (2026-09-26), the L8 scale slice's repair round 1 (`b1b03e0a..c4557587`, the documentation commit that records this entry and the CLAUDE.md commit after it).** Three reviews of rev 3.30 (tests, safety, consistency) raised 22 findings — 1 major, 12 minor, 9 nits; the tests and safety lenses both raised the row-derivation hazard. The lead passed on 16 (1 major, 12 minor, 3 nits), and each is fixed, the behavioural ones red then green, the pins each with a negative control (the count corrected by rev 3.32). Not in this round: the tests lens's pin on the daemon's wiring of `open_default_deferred` and `spawn_memory_index_rebuild`; the safety lens's F4 (`index_serves()` sampled once per read — answered for the reads that wait by `f757df76`, below) and F5 (a stop during a long rebuild keeps the process and its singleton lock past `STOP_TIMEOUT`, 15 s — an existing class: before S1 the first tick's whole replace did the same); and the consistency lens's N1 (rev 3.30 files bulk stat under the owner decisions, though it was left out only because the spec did not list it), N2 (the file scan has the index's filters and ranking weights but no BM25, and recovery's sweep is re-armed by every index rebuild and a workspace move too) and N4 (the documentation commit `29fa2b93` carries two lint fixes, so `e8805225..c7574587` each add a clippy warning). Rev 3.30's S5 and S6 bullets are corrected in place where this round changes them. **No frozen contract (03 §7a) and no ruling (03 §7b/§7c) changes;** 03's rev 7.3 records the two §5 mechanisms that now hold as written.
  - **The idle merge merges** (`b1b03e0a`, tests, major): `maintain` sent FTS5 `('merge', +256)`, and a positive merge only merges a level holding `usermerge` (4) segments, which automerge never lets build up — every step was a no-op. It sends `-256` now (every segment on one level, as few as two merged, still bounded per step). The storage test asserts `merge_steps > 0` and one segment per index after it (`count(DISTINCT segid)` of `fts_word_idx`/`fts_tri_idx`), and the service's idle-tick test (1, 1) where it left (2, 2). `rusqlite` is a dev-dependency of the core crate from here, so a service test reads the cache file.
  - **A panicked rebuild holds no reader** (`397a5c42`, safety): `running` was cleared only after `rebuild_index_inner` returned, so a panic left it set for the life of the process and every `touch` waited for good. A `RebuildRun` guard now clears it on drop, leaves the rebuild owed unless it completed and wakes every waiter.
  - **A read overlapped by a rebuild reads again** (`f757df76`, tests): `touch`'s wait was check-then-act — a rebuild starting after it emptied the file under the caller's next reads. The rebuild state counts rebuilds started; `MemoryService::read_index` runs a touch and every read after it and, when the count moved, waits for the rebuild and reads again (learning's three lookups, the timeline, the suggestions list, a deleted conversation's summaries); `pending_suggestions` lists nothing instead. The new test holds a timeline after its touch, starts a rebuild under it and was red (it answered from the emptied index); a second pins the wait itself.
  - **Damage met by any read owes a rebuild** (`b74f50a4`, safety): only `touch`, recall's and search's own search, the index passes and the tick marked a corruption error; every other read passed it up unmarked, and some B-trees are reached by those reads alone, so damage there was never rebuilt, a restart included (open runs no integrity check since S6). One helper, `MemoryService::indexed`, takes every request-path index answer and marks damage before the error goes up. Tests damage one B-tree's root page and were red: `chunk_note` (a pinned note's chunk count in recall) and `note_source_key` (learning's `notes_citing`). **Rejected in part, with evidence:** the finding's example, `note_partition_pinned`, is the index SQLite's plan uses for the stat scan's own query (`EXPLAIN QUERY PLAN`: `SEARCH note USING INDEX note_partition_pinned (partition_key=?)`), so `touch` already met and marked damage there; the fix stands, on the two B-trees above.
  - **Pins** (`1a592b09`, tests): S1's equivalence — a seeded random run of saves, replacements, learned notes on shared rows, suggestions and their confirmation, forgets (one flipping a sibling's `source_forgotten`), archives, hand edits and deletions and restarts ends, at four checkpoints, with an index whose every note column, chunk, alias and citation equals a separate rebuild's, both FTS indexes passing `integrity-check`, and no rebuild run meanwhile (negative control: a fingerprint blind to `source_forgotten` fails at step 79); S3's history sweep at the tick, and the read-only partition's sweep deleting nothing; the read-only `doctor` leaving a forgotten note's file and `.history` on disk (each negative control fails).
  - **S7's tests fail rather than hang** (`f993b83c`, nit): an outer 5 s bound on each stalled call; the suggestions test seeds one pending suggestion, sees it rendered unheld and not held, and checks the WARN (negative control: with the bound an hour, both fail in 5 s).
  - **A forgotten file that cannot be deleted is logged without its title** (`790dff5e`, consistency): the WARN printed the file's path, whose slug is the forgotten title, at every scan; it prints the id and the operating system's error (red test: a read-only `notes/`).
  - **Wording** (`c4557587`; safety and tests on the hazard, consistency on the rest): `SCHEMA_VERSION` versions what a row holds — since S1 rows survive restarts, so a change to the chunker, `query::normalize`, `authority_rank`, `record_for`, `date`/`norm_title` or the tokenizers must bump it (CLAUDE.md's memory-search gotcha says so too); `reindex_after_write`'s doc and WARN; `replace_partition`'s doc; the bench's `open_default` comments; the boot-rebuild task's failure message; the CLI Manual's "only" (a bloated cache is rebuilt too) and the Daemon Manual's period and one-shot boot rebuild. 03's status line credited rev 7.1's text to rev 7.2 and is rewritten with each revision's own words (now rev 7.3). CLAUDE.md's five moved citations and its memory paragraphs follow in their own commit.
- **rev 3.30 (2026-09-26), the L8 scale slice: the memory service's no-contract-change fixes (`d0d487ba..c7574587` and the documentation commit that records this entry).** The L8 benches and their profiled analysis — with the sceptic review that corrects it and names this slice's set (O1, O2, O3 with the scan-time check, O8, O10, M1, M2, M4, M7; the spec is `scale-fix/SPEC.md` in the session scratchpad) — found the 100K costs almost all per-operation work over the whole partition. One commit per item, each behavioural change tests first (red on `c3979ba5`, then green; the pure-cost items pinned by equivalence tests that pass before and after). **No frozen contract (03 §7a) and no ruling (03 §7b/§7c) changes;** the three mechanisms of 03 §5/§6 that change shape are 03's rev 7.2.
  - **S8** (`d0d487ba`): `derive`'s `source_forgotten` keys each suppressed source to the latest forget that suppressed it, once per derive — linear in the cited sources, not sources × tombstones; the same answers (a 400-note, 1,000-tombstone equivalence test against the rule restated naively).
  - **S2** (`5ee3a0e5`): recall's per-hit verification and learning's `notes_citing` reads refuse a tombstoned note through `Partition::is_tombstoned` / `with_snapshot`, in place — each used to copy the whole snapshot (8 copies for one recall of the test fixture, 0 now).
  - **S4** (`e8805225`): a write's index stamp is `MemoryIndex::stamp_scan` with the view's own file count, not a `reconcile` that read every row back (O10); `Parsed::put` finds the replaced files by binary search and re-counts duplicates only for the ids it touched (O8), pinned step by step against the old whole-list `put` over 400 random writes.
  - **S1** (`5c99bb24`): a partition's first index pass in a process reads its fingerprints from the index rows (`MemoryIndex::row_facts`; every fingerprint field is a `note` column), so the first write or tick after a restart, `touch`'s stat difference and every dirty hit re-index only what changed — the `full` flag is gone, and the whole rebuild stays in `rebuild_index` (missing, recreated or schema-bumped cache). Rows built: 9 → 2 for a restart's first write, 6 → 0 for its first tick, 10 → 1 for an external edit.
  - **S3** (`f5cd2223`): every scan — `scan()` before a read and at every tick, `parse_all` behind every listing, write, index pass and `MEMORY.md` — leaves out a file carrying a forgotten id (by its name, or by its frontmatter under another name) and, opened for writing, deletes it with its `.history`: a forgotten note put back mid-process (a backup restore, a sync client) is never listed, indexed, re-recorded `external` or navigated, which recovery at open alone did not guarantee. Recovery's sweep then runs once per partition per process through the service's cache and again after a forget that failed after its tombstone; the tick deletes a forgotten id's `.history` put back on its own (03 rev 7.2 says why this is exactly as safe). `doctor`'s read-only open leaves such a file out and deletes nothing.
  - **S6** (`cc2552a8`): the index opens with no `quick_check`; the daemon opens with `MemoryService::open_default_deferred` and rebuilds a missing, invalid or schema-bumped cache on a blocking worker (`background::spawn_memory_index_rebuild`), every tick retrying one still owed; the rebuild recreates the file and indexes in 256-note transactions; until it is done `recall` and `search` answer from a bounded scan of the files (`MemoryIndex::scan_search`/`scan_pinned`: the index's filters and ranking in memory, at most `FILE_SCAN_MAX_NOTES` = 10,000 newest notes, `rebuilding`, `partial` when capped), `pending_suggestions` lists nothing, other index reads wait while it runs (and, from rev 3.31, read again when one starts under them), and index passes are skipped (a write it missed is found by the next stat scan). Damage met on use owes a rebuild the next tick runs (met by any request-path read from rev 3.31; here only by a stat scan, a search, an index pass or the tick). `open_default` (tests, the evaluation harness) still returns ready.
  - **S5** (`df2bf220`): `replace_partition` ends with `optimize` and `wal_checkpoint(TRUNCATE)` (a path no service calls since S1, rev 3.31 notes; only the index's tests do); connections set `journal_size_limit` (32 MiB); `MemoryIndex::maintain` merges FTS segments within a budget and truncates the WAL (it merged nothing until rev 3.31: a positive FTS5 `merge` never found a level to merge); the tick runs it once the index has sat idle for a tick, and rebuilds (from a new file) a cache grown past `INDEX_BLOAT_FACTOR` = 2× its size after its last rebuild — measured: a quarter of 2,000 notes edited six times over grows the file 1.39×, a whole rewrite 1.80×, a doubling of the notes 1.78×.
  - **S7** (`2b43abfa`): the per-turn recall and the pending suggestions are bounded by `REPLY_MEMORY_DEADLINE` = 1.5 s (a constant): past it the turn continues without them, as on an error, with one WARN naming the partition keys and the elapsed time and no note text. Chosen from the benches: over twice the slowest healthy recall p95 (661 ms at 100K average notes), under every unbounded wait found (2.9 s and up).
  - **S9** (`c7574587`): the service bench seeds tombstones on request, and `bench_service_10k_notes_one_tombstone` is the sceptic's M9 baseline; every run also prints the daemon's deferred open. The benches are re-run by the lead on a quiet machine (`l8/bench-after/`); this entry records no measurement of its own beyond S5's bound.
  - **Not built — owner decisions** (the spec's list): O4 (no stat scan before every read, or a watcher), O6/O7 (management reads and `find_note` from the index), O9 (a debounced `MEMORY.md`), O13 (`pending` out of the snapshot), O14/M8 (parse-cache sizing), O15/O16 (incremental partition state, an append-only control log), O17 (the trigram layout or a document-frequency cutoff); and bulk stat, which the sceptic grouped with this slice but the spec did not list.
- **rev 3.29 (2026-09-26), the L8 fix slice's nits round (`4fc3ba50..5dc9025e` and the documentation commit that records this entry).** Three reviews of rev 3.28 (tests, measurement fidelity, consistency) raised one minor finding and six nits; the lead passed on the minor finding, the fidelity review's lag nit open since round 2 and the six nits (one of them, H6's link-check totals, a harness change), each now fixed. The behavioural changes are tests first — red, then green, all offline — each with a negative control. No learning behaviour a user sees changes.
  - **A summary's per-window retry, pinned** (`4fc3ba50`, tests F1): rev 3.28 bounds a summary's retries per position of the episode cursor, but no test failed without that reading — the summary tests used one window, where a constant position behaves alike. A session whose summary window spans two chunks, the first never usable, now pins the attempt stops `[BadOutput ×3, null]`, 4 attempts, 5 calls, one window skipped and one summary; it fails with the position a constant and with the learning cursor read instead (a summary never moves it).
  - **A lagged subscription is counted, totalled and warned about** (`81534945`, fidelity N1): H3's surface split and H7's restatement attribution are read off the bus — a lost `created` counts a flush's note as the reader's, a lost reader write takes a restatement's `model_tool_write` away — but the fed turns' and the question turns' subscriptions passed over `RecvError::Lagged` uncounted, and the flushes' `events_lagged` was never totalled. Each subscription counts what it lost (`learning.turns[].events_lagged` and `rows[].events_lagged`, beside the flushes' and the summaries'); `learning.cost.events_lagged` and `events_lagged_at` total and name the learning phase's, and one `learning.warnings` line says what may undercount; the forced re-run's stays in its own record, as its other counts do, and a warning follows it, since its resurrection check reads its flush's `created` events; `totals.events_lagged` and `events_lagged_at` total the question phase's, printed as a warning under the totals. The bus holds 1,024 events, and no flush, summary or forced re-run of an archived L8 report recorded a lag (the turns' and the questions' were not counted then). **Reading:** a re-run whose subscription lagged still reads as checked (`forced_rerun_checked`); counting it unchecked would be one more branch in `forced_rerun_unchecked`, and is not taken here.
  - **H6's link checks, totalled** (`5dc9025e`, fidelity n3): a check is one more coverage-judge call, and its tokens and verdict were kept only per pair, with a check that ended unparsed or in a judge error printed like a `NONE`. `learning.link_checks` counts the checks, the calls, the pairs credited, the verdicts — `matched`, `none`, and apart from it those that `failed` with no verdict (`unparsed`, `unparsed_cut_at_max_tokens`, `judge_errors`) — `not_judged`, and the judge tokens, which `coverage.judge_tokens_*` leave out; the printed line carries them.
  - **Documentation** (consistency n1–n3, fidelity n1 and n2): the runbook's Cost paragraph (the retries are per chunk; H6 adds at most 8 coverage-judge calls a run); its `learning.changed_facts` row (a check's pick is scored by the base rule — any superseded note or same-key loser credited — and `old_note_from: link_check` is recorded only when the check named a note); the `learning.flushes` row's labels without `forced_rerun`, and a `learning.forced_rerun` row; the cost counters "over every attempt of the learning phase", the forced re-run's being in its own record; rev 3.26's H2 bullet points to rev 3.28, and rev 3.28's H6 outcome list and `old_note_from` sentence are corrected in place.
- **rev 3.28 (2026-09-26), the L8 fix slice's repair round 2 (`d2a1bad6..cc424f1d` and the documentation commit that records this entry).** Three reviews of rev 3.27 (tests, measurement fidelity, consistency) raised, for this round, four minor findings and two nits, each fixed; the behavioural ones red then green, all offline, each with a negative control. Two more one-line nits of the consistency review are fixed in the runbook (its list of the slice's tests, the empty completion's reason) and its third is settled below. The fidelity review's nit on a lagged bus subscription (`events_lagged` is not totalled or warned about) was not in this round. No learning behaviour a user sees changes.
  - **Which flushes are retried at once, pinned** (`d2a1bad6`, tests F1): repair round 1 split H2's retry by label and only `turns` and `switch` had a test that fails without it; a table test now pins `turns` false and `switch`, `replay` and the forced re-run true.
  - **A final flush reads or skips every window** (`bfcca399`, fidelity M2): H2 capped a trigger at `MAX_ATTEMPTS` attempts in all, but the queue counts unusable answers per cursor position and, on the third, skips that chunk and goes on to the next in the same call — so a flush of two chunks whose first was never usable ended `BadOutput` with the second held after one answer and, the session having no later flush, never read. A flush other than `turns` is now retried until the product has read or skipped each window (at most `MAX_ATTEMPTS` attempts stopping at one position — the learning cursor, or the episode cursor for a summary); `learning.cost` counts the sessions whose last learning or summary flush ended with any stop (`windows_left_unread`, `episode_windows_left_unread`) and `learning.warnings` names each with its stop. The runbook and `retried_at_once`'s doc now say how the stand-ins' pace differs from production's (fidelity N2): a `turns` window is retried at the session's next tenth turn or its switch where a daemon retries one to three turns later, so it is read grown further or, failing, skipped larger (the bias's direction unknown); a finished session's window is retried with a rebuilt known-notes section by a daemon 60 s or more later, so the immediate retry is the stricter.
  - **H6's link is judged** (`cc424f1d`, fidelity M1): the link credited a pair on a search hit no judge had compared with the old gold — it may be an earlier version of the new value, and an active note that still states the old value may sit beside it (L's `user.commute` had one; L's `moss.output_dir` lists f34ccadd). The link now makes a check due: the coverage judge is asked about the old gold once more, with the frozen prompt and its own cap, shown its hits without the new note, and the pair is scored from that pick as any pair is (a superseded note, the linked one or another: recorded; a same-key loser: recorded; any other active note: `both_active_unlinked`; `NONE`, no verdict line, a failed call: `old_never_learned` — worded by rev 3.29). A checked pair is one whose `link_check` is set; `old_note_from: supersedes_link` is now `link_check`, recorded only when the check named a note (`judge` otherwise, the old note being the new one — rev 3.29). At most one call a pair. **Re-score:** offline the check's pick cannot be replayed. Each archived link has one check due — L's `moss.output_dir`, L-episodes' `user.laptop`, the `user.commute` of replays 1 and 2 — and in each the superseded note is candidate [1] and the one that states the old value; L's also lists f34ccadd, an active note on previewing the build under `_site/`, a related fact the prompt's rule 1 does not match. So rev 3.26's re-scored figures (L 11/15, L-episodes 13/15, replays 8 and 9) stand if the judge names the superseded note, and the floor is the base (L 10, L-episodes 12, replays 7 and 8).
  - **Documentation**: `replay_is_extraction_only`'s doc and the H4 test's say what the runbook says — a replay compares with a live run's per-note `extraction` shares only (consistency m1); the fit test's per-size p90 table is rebuilt from the sample its comment names (p3_fit's 230 flushes: 307 at one item and 528 at two, where it had 308 and 526 from a sample that took smoke-L in and left the forced re-runs out; no assertion changes — tests N1).
  - **Rev 3.27's own list** (consistency n2): rev 3.27 corrected rev 3.26's P3, H2, H3, H4, H5, H6 and H7 and its commit range — rev 3.26's header names P3, H2, H4 and H7, and rev 3.27's text P3, H2, H7 and the range. H7's "no fed flush read the turn to the end" is rev 3.26's rule; since rev 3.27 it is "no fed flush moved the cursor across the turn's row without a skipped window", and `turns_fed` is read by no classifier.

- **rev 3.27 (2026-09-26), the L8 fix slice's repair round 1 (`98ba8d75..c4b21f6a` and the documentation commit that records this entry).** Three reviews of rev 3.26 (tests, measurement fidelity, consistency) raised 18 findings — 5 major, 9 minor, 4 nits; the `turns` retry and the read range were each raised by more than one lens — and each is fixed, with three more nits the reviews noted (a stale `learning.episodes` row, H6's rule missing "superseded", no word that H6/H7 reclassify); the behavioural ones red then green, all offline. No learning behaviour a user sees changes but P3's budget, re-derived. Rev 3.26's entry is corrected in place where it was wrong (P3's ruling, H2's skip wording, the H7 tally's labels, its commit range) and says where rev 3.27 narrows it.
  - **`turns` is not retried at once** (`98ba8d75`): H2 re-sent a `turns` flush that stopped `BadOutput` with the same request, and at temperature 0 the third answer skipped the window for good; production holds it under its backoff and mid-conversation reads it grown — in L8 three such windows were read cleanly by their session's switch (L cs-006, L-episodes cs-026 and cs-035). The session's next flush is now the retry; the queue counts its attempts on from the held one. `switch`, `replay`, summary and forced re-run flushes are still retried at once.
  - **Read by range** (`9ffa5af9`): each flush records `cursor_before`/`cursor_after`, and H7's "extraction read the turn" means a fed flush with no skipped window moved the cursor across the turn's user row — a flush reads from the cursor, so a later clean flush never read a window an earlier one skipped. A flush that skipped a window counts none of its range read (conservative, in a multi-chunk flush only). The archived re-score is unchanged: no L8 flush skipped a window or stopped after reading a chunk.
  - **Failures counted** (`36976891`): `learning.cost` adds, for learning and for summaries apart, `flush_attempts`, `unusable_answers` by reason (`cut at max_tokens`, `empty completion`, the parser's reason) and `windows_skipped_unusable`; a summary record keeps `skipped_unusable`; `learning.warnings` names each session whose window was skipped. A retried record's `stopped` is its last attempt's, so failures are counted from these.
  - **Latency over attempts** (`868a5237`): each attempt is timed (`attempt_latencies_ms`) and the flush p50/p95 read over attempts, one per flush a daemon would run.
  - **Coverage totals pinned** (`af808f56`): the stub model can report its output tokens; H5's test asserts all seven totals; two negative controls fail it.
  - **A replay is labelled** (`4a045bfd`): its printed totals say `L (a replay of …: extraction-only store)`; `compare.py` labels it `(system L, replay)` and warns.
  - **P3 re-derived** (`c4b21f6a`): see rev 3.26's P3, now the fitted line's band (216 an item, a 2,892-token ceiling; 8K eight items).
  - **The runbook**: live coverage and noise by path are the judge's single pick under reader pre-emption, so a replay compares with a live run per note only; coverage across H5's boundary is bounded by the old report's `unparsed`; H6 and H7 reclassify, so new figures compare with L8's only through rev 3.26's re-scored ones; a replay's summaries are uncapped where L8's L-episodes had 27 stopped; and doc comments that predated H8 and H9.
- **rev 3.26 (2026-09-26), the L8 fix slice: the evaluation harness measures what it claims, and extraction failures can be seen (`8fa9354d..d579bcf9`, the documentation commit `76a37edb` among them; its review's repair round 1 is rev 3.27, which corrects P3, H2, H4 and H7 below).** Its source is the L8 diagnosis — five findings, each with two sceptics' corrections, in the session scratchpad (`memeval/l8/diagnosis/SUMMARY.json` and its notes: the forced re-run's BadOutput, L-S's suggestions, the episode split, L's gap to D-index, the completeness sweep) — and the lead's ruling of 2026-09-26 that turned it into this slice. No learning behaviour a user sees changes, but P3's budget; the design gaps the diagnosis raised (a model-tool note that pre-empts a suggestion, the extractor's key reuse, tolerant JSON parsing, what the episode prompt keeps) are the owner's and none is implemented. Each item red then green, all offline; 03 is not folded, and §1.6's formula and §1.8's "≤ 2,800 output tokens" are superseded by P3 below, left for the lead.
  - **P1** (`8fa9354d`): the queue never read a response's finish reason, so an extraction or summary answer stopped at `max_tokens` failed as "the answer is not JSON". When the parse fails and the response finished at `MaxTokens`, the reason is `cut at max_tokens (<n>)`, n the request's ceiling — same `BadOutput`, same LR20 count; a cut answer that parses is still used. `llm_call_log` keeps its fixed "JSON parse failure".
  - **P2** (`27aa0682`): `FlushReport` and `EpisodeReport` carry `unusable` — per unusable answer its reason, length in characters, finish reason (`None` for an `EmptyCompletion`) and first and last 400 characters, character-safe — in memory only: nothing new is persisted, and the excerpt is logged at DEBUG and nowhere above. The harness copies it into each record's `unusable_answers`.
  - **P3, the ruling** (`39974e27`, re-derived by rev 3.27 in `c4b21f6a`): §1.6 assumed 180 output tokens an item under a 2,800-token ceiling. Over L8's 230 successful single-call flushes (L, L-episodes, L-S and the three replays, forced re-runs among them) an answer of n items cost 83.6 + 179.5 n tokens (least squares; p95 residual 182); an item's cost over that intercept had a p90 of 216; the answers of five or more items ran at most 241 tokens an item (p90 221); no answer held more than 10 items. So 180 was the fitted slope with no band above it, and twelve items at the worst large-answer rate (2,892) did not fit 2,800. The ruling: an item is budgeted at the p90 marginal cost (`ITEM_OUTPUT_TOKENS` = 216); the JSON keeps its 300-token allowance (`ANSWER_OVERHEAD_TOKENS`, at least the intercept plus the residual, 266); the ceiling is that allowance plus `extract_max_items_per_call`'s default twelve items (`MAX_OUTPUT_TOKENS` = 2,892 — also 12 × 241), so `out_tok = min(2892, window / 4)`, `items = min(extract_max_items_per_call, (out_tok − 300) / 216)`. The band covers the p90 of every answer size L8 saw and all but two of its 171 compliant answers with items (a 1-item answer of 1,039 tokens, a 4-item one of 1,294). A window of 11,568 tokens or more keeps twelve items; a 16K one twelve and 17,488 B of transcript (17,617 at BASE); an 8,192-token one — also a model whose window is unknown — eight (nine at BASE); a 4K one three (four); below 2,064 tokens the one-item floor is over the band. The slice's first cut (`39974e27`) budgeted an item at 320, the highest per-run p90 of tokens ÷ items per answer, which for a small answer already holds the JSON, and added the 300 on top — counting it twice: 8K five items, 16K eleven, a 4,140-token ceiling that rescued no compliant answer (the 8 answers cut at 2,800 were runaways); the fidelity review measured the double count. `the_largest_answer_the_prompt_permits_fits_the_ceiling` pins the band against the fitted line with its residual, the worst large answer and the measured p90 per size; `budget_for_an_8k_window` the 4K, 8K, 16K and 128K budgets.
  - **H1** (`bbd94e7d`): a learning system's config lifts `[orchestrator.learning] episode_max_per_day` to the validation's maximum, read from `DaemonConfig::validate` (1000; never 0, which writes none) — at its default of 20 it stopped every summary from the 21st session on — and records it; `learning.warnings`, printed first, names any flush a daily cap stopped.
  - **H2** (`b6aae718`): in `Manual` mode a flush that stops `BadOutput` is flushed again, up to the product's `MAX_ATTEMPTS` (for each chunk since rev 3.28, which see), with no backoff and the same request, each record keeping its attempts and their stops. **Deviation:** it applies to a replay's whole-session flush too (it stands for the switch; the ruling listed turns, switch, summary and forced re-run), so replay and live stay comparable. Since rev 3.27 a `turns` flush is **not** retried at once: its session's next flush is its retry, as production's backoff reads the window grown. The sceptics' measurement is recorded, not worked around: at temperature 0 the same prompt failed the same way in 13 of 13 pairs, so a retry mostly ends in the queue's own skip, and nothing perturbs the prompt. Rev 3.11/3.12's rule stands and the retry's false pass is closed: the `MAX_ATTEMPTS`-th unusable answer skips the window's chunk and the flush goes on, ending with no stop when no chunk holds it, so a re-run with a skipped window is unchecked too; `forced_rerun_unchecked` says why ("unchecked after N unusable answers"), apart from a resurrection.
  - **H3** (`bf094ca8`): coverage matches, aliases, fact keys, noise and restatements are reported for `extraction` and `model_tool` beside the pooled figure, which is unchanged (the offline re-score reproduces every archived pooled number first). A learned note is `extraction` when a learning flush's `memory_changed` created it and `model_tool` otherwise (`learned[].surface`) — while the corpus is fed the reader's `memory_store` is the one other writer of an `extracted_*` note. Coverage counts the judge's single pick, so a gold both paths learned counts once, for the note it named (L: 34 of the reader's 58 matches also had an extraction note among the hits), and the note passed over counts as its path's noise — a live run's coverage and noise by path are attribution under the reader's pre-emption, not a path's own (rev 3.27). L: other-script aliases 83.3 % extraction (30/36), 18.8 % model_tool (15/80), 38.8 % pooled; coverage 12 and 58 of 70.
  - **H4** (`c0349703`): `conditions.learning.replay_is_extraction_only`; the runbook says the replays' spread is the extraction path's variance, compared with a live run's per-note `extraction` shares only (aliases, keys — narrowed by rev 3.27: live coverage and noise by path are not comparable, L's live extraction coverage of 12 of 83 against the replays' 60–66 being attribution and pre-emption), and that a live run is the only sample of the whole system. Reader writes are not re-applied in replays (it needs a live feed that records them). §7.3's "a target is read against the median" of the four samples is set aside in the runbook until the owner answers which system a target applies to (learning-gap's owner decision 1); folding it into §7.3 is the lead's.
  - **H5** (`01eb45bb`): the coverage judge asks for 2,048 tokens (`COVERAGE_JUDGE_MAX_TOKENS`, recorded); at the QA judge's 400, which it keeps, every L8 unparsed coverage verdict (25 of 492 calls) was a deliberation cut before its line, and the longest verdict that ended on its own was 367. The totals count `none`, `unparsed` (and those cut at the cap), `not_found` and `judge_errors` (which also counts a failed search) apart. The frozen prompt is untouched. Across this boundary coverage can gain up to the older report's `unparsed` count from the cap alone (L8's L 2, replays 5–8: up to +9.6 pp) unless its store is judged again; the runbook says so (rev 3.27).
  - **H6** (`5bf0eda9`; the link is judged since rev 3.28, which see): when the judge matched one note for both golds of a pair and that note is active, no same-key loser, and supersedes a superseded note the old gold's search returned, the pair is `new_active_old_superseded` (`old_note_from: supersedes_link`). **Reading:** the rule as the sceptics' numbers require; "a single active note matched as both" without the link would also move L's and L-episodes' commute and L0's two pairs (L 12/15). Re-scored: L 10 → 11/15, L-episodes 12 → 13, L-replay-1 7 → 8, L-replay-2 8 → 9. The pooled pair key reuse is read over the linked old note, so it moves too (recorded by rev 3.27): `pairs_key_reused` of `pairs_with_two_notes` L 1/4 → 1/5, L-episodes 2/5 → 2/6, L-replay-1 6/6 → 7/7, L-replay-2 5/5 → 6/6.
  - **H7** (`751ee824`): where no note cites a restating row, it is `model_tool_write` when the reader changed a note during the turn (the turn's `model_tool` events, now kept as `model_tool_notes`), else `extraction_failed` when no fed flush read the turn to the end (`turns_fed` marks a fed flush's place; a skipped window is no read — decided by the cursor's range since rev 3.27), and `nothing_written` only for a turn extraction read; a run with no flush (L0) fails nothing. Re-scored: L 14 `nothing_written` → 11 + 3 `model_tool_write`; L-episodes 13 → 3 `nothing_written` + 5 `model_tool_write` + 5 `extraction_failed`; L0 15 → 10 + 5; each of replays 1 and 2 one `extraction_failed`; L-S 5 → 2 + 3. The §7.3 target's suggestions and duplicates do not move.
  - **H8** (`679f03fd`): `learning.cost.feed_turn_errors` and `feed_turn_errors_at` (L0: 7, cs-026#8–10, cs-027#0–3), printed and warned about.
  - **H9** (`1b199807`): a replay with `OPENALPACA_MEMORY_EVAL_QUESTIONS` naming a file writes each session's summaries (label `replay`) and asks the questions as D asks them, learning off, before its forced re-run; without it a replay is unchanged. Not like for like with L-episodes: extraction-only store, L's transcript.
  - **H10**: the runbook describes all of it.

- **rev 3.25 (2026-09-26), private use joins the drop.** §4.4's list of what the preview drops names private-use code points: the escape set gained them (the hidden-characters spec's A32, `940a5545`), because Apple's system fonts draw some of them as nothing, so the preview drops them and its count line counts them. No behaviour change here.
- **rev 3.24 (2026-09-26), the approval-surface hidden-character work recorded.** §4.4 gains the preview's contract as built — the five served fields it quotes (the 600-byte excerpt as "Yours:"), the `needs_escape` drop, A18/A19's count line and the 216-byte budget under it — which it described none of (the S4 round-2 and round-3 consistency reviews' n1/n3). §3.5's API line gains `knowledge_left_out` and `target.excerpt_left_out`, which the Suggestions card prints as `... (N more characters not shown)` (the S4 round-3 consistency review's m2; `9fd1b4ac`). No behaviour change here; the commits are named in the hidden-characters spec's S5 record.
- **rev 3.23 (2026-09-25), the documentation pass's fact-check review.** §4.1's result line gains the `invalid_input` refusal the tool gives before any card — a target no longer effectively active (rev 3.6's S5 check, in `suggestion_preview` too) or a malformed `expected_revision`/`target_revision` (`check_revisions`) — mirrored in 03 rev 7.1's §7a. §1.6's citation for the default 40,000 TPM is `default_tpm` (`routing/rate_limiter/mod.rs:415-417`); `:410-412` are `default_rpm` and the end of the retry field. No behaviour change.
- **rev 3.22 (2026-09-25), the lead's documentation pass after the residue slice (the commit that records this entry).** No behaviour change. The status line names every slice, its review fixes and the entries that record them, and says L8 is running; it no longer says nothing is built until the owner approves. §1.6's rate cap reads `TPM × 1.6 − REQUEST_OVERHEAD_BYTES` (56.5 KiB at 40,000 TPM) with the ~3,000 TPM floor. §4.1's description sentence and §12.2's refusal sentence say what the code says after the residue slice's R3 and R5; §12.3 says a memory command is parsed before skills and, on the production path, before task control. §10 gains **LQ5–LQ10**, pending, none adopted. §11 says 03 rev 7 folds it and gains rows for the undo response and S5's retired target (L3), `preview`/`per_call` (L4), L10a, L10b, L10c and residue R6. Rev 3.15's *For L8* is marked superseded by rev 3.20, and rev 3.6's finding 3 notes that R6 is what fixed an index error after a landed write. What the residue slice changed, each red then green, `4f2eb5e5..da53e1a2`:
  - **R1** (`4f2eb5e5`, L5 review 1): the recall header prints `timing=upcoming` only when the index row is the file the re-read found (not stale), at the hit site and the pinned site.
  - **R2** (`3a0f9d7f`, L4 re-review): `memory_confirm`'s preview, the `<memory_suggestions>` lines and the chat commands' lists drop Unicode format characters (Cf) as well as controls — one helper, `utils::text::one_line`.
  - **R3** (`d124b930`): `memory_confirm`'s description states the lane rule the code applies (§4.1).
  - **R4** (`095dea95`, L9 nits): comments only — an id-shaped word starts with any hex digit; `IN_CHAT_SOURCE` and `ATTENDED_CHAT_SOURCES` name each other.
  - **R5** (`63e13579`): the memory-command refusal names the terminal it needs (§12.2).
  - **R6** (`7d5174e9`, L3 re-review): an index failure after a landed write is logged and the write answers `Ok` with its `memory_changed` (store, update, forget, withdraw); the partition's fingerprints are dropped so the next pass re-indexes it whole (§11's last row).
  - **R7** (`026d5de0`, L1 re-review): learning and the episode writer re-check the project root right before they write (LQ5).
  - **R8** (`8819d7ee`, then `6038ca53` and `da53e1a2` from its review and re-review): a pre-existing OpenAI defect, not Learning v2's — every builtin marked `strict` went to OpenAI (and to Ollama through the same builder) with `"strict": true`, and 20 of the 21 schemas break strict mode's rules, so the API would refuse the request. The flag is now sent only when `openai_tools::strict_mode_compatible` accepts the schema: closed objects with every key required, an allowlist of documented keywords each in its place, and a `$ref` standing alone. Only `memory_confirm` keeps it today; no schema was edited.
  - **The CLI's memory output** (`11d4e48c`, the residue review's finding 3): `openalpaca memory` prints note titles, reasons, aliases, tags and excerpts with no control or format character, and `show`'s body with no control character.

- **rev 3.21 (2026-09-25), §7.1's second-session corpus review applied (the commit that records this entry).** An independent read-only session reviewed `conversations-learning.json` and `questions-episodes.json` against the gold set, the held-out questions, the extractor's prompt and window and the harness's feed and L-S seeding: **2 blocking, 6 should-fix, 11 notes; every blocking and should-fix edit is applied, verbatim from its proposed file**, and nothing else in the corpus changed (a structural diff shows only `user` strings plus one inserted turn).
  - **B1 (blocking): relative dates.** Nine turns gave a date the gold Knowledge carries only as "this month", "today", "yesterday" or "这个月" — or, in cs-020, not at all (cs-013, cs-015, cs-020, cs-024, cs-029, cs-033, cs-037, cs-040, cs-043). The extractor's `<today>` is the chunk's last row's wall-clock date (`memory/learning/window.rs`), so a learning run would have stamped the run's own month and the coverage and QA judges would have failed L on the corpus, not on learning. Each now states its absolute date, equal to the session's `narrative_date` or the gold Knowledge.
  - **B2 (blocking): a fact outside the gold set answered an abstention question.** cs-051[1] and cs-049[2] gave the team's size ("eight people"), which answers hq-0418 (`expect_abstain`); both are removed.
  - **S1–S4 (should-fix), the corpus.** S1: five turns now carry the record dates the gold Evidence carries (cs-001, cs-004, cs-014, cs-022, cs-031). S2: cs-032's partner name `acme` (hq-0411 is an abstention) is now `<partner>`. S3: cs-050 gains a restatement that issue #212 is still open in September (hq-0279), before its last turn. S4: cs-003's "15 December" gains its year.
  - **S5–S6 (should-fix), the episode split.** eq-004 asked what a gold note answers (MongoDB turned down, PostgreSQL chosen); it now asks for the app and what was sketched. eq-007 now names the conversation that *first* noted the staging database, `lantern_staging` on `pg-staging-1`, since cs-042 notes it too. `turns` are unchanged. N9 (trimming the long episode answers) is **not** applied.
  - **N1, left deliberately.** hq-0276 asks when the Lantern retry policy was decided; neither gold `lantern.retry_policy`'s body nor the corpus dates it, so D′ and L both miss it. Dating it in the corpus alone would favour L over D′, so it stays undated. The other notes (N2–N11) need no edit.
  - **Rev 3.10's corpus figures move:** 299 → 300 user turns, 14 → 15 restatements (still 6 of the pairs' old notes), noise 55 % → 54 % (163 of 300). Sessions, languages, workspaces, the 23 notes stated only in passing, 7 corrections, 11 rejections and 13 suggestion sessions are unchanged. The README says so. No test pins an exact count (`memory_eval_corpus.rs` checks §7.1's ranges), so no test changed. A mechanical guard for B1's class (a relative-date word with no date in the same turn) was prototyped and not adopted. In the old corpus it flagged seven of the nine B1 turns and three of S1's. It also flags three habitual phrases that are not dated facts ("这周五…照常", "这周日照常", "上周任务"), which it still flags after the edit. And it misses two B1 turns: cs-020, whose refusal had no date word at all, and cs-040's "yesterday", whose turn names 2027. The date check stays a reading, not a test.

- **rev 3.20 (2026-09-25), L8-prep: the harness can run D in both recall modes (§13.4), closing rev 3.15's *For L8* (`bee1bd27` the harness, `77508902` compare.py, `f24e4573` the README, and the documentation commit that records this entry; 03 is not folded, and no production code changed).** Readings taken while building it, each recorded here rather than silently:
  - **The switch.** `OPENALPACA_MEMORY_EVAL_RECALL_MODE` = `index` | `excerpts` sets `[orchestrator.memory] recall_mode` in `eval_daemon_config`, the one config every system of a run is built from (D, E, L, L0, L-S) and a replay; unset keeps `DaemonConfig`'s default, `index`. Any other value is refused as the first check of `Settings::from_env`, before the router exists, naming the two values; the spelling is the config key's (`Index` is refused). `conditions.recall_mode` records the value used.
  - **Listed is not delivered.** `relied_notes_delivered` keeps the archived excerpts-era meaning — the note's content reached the model, any `memory_used` reason but `index` — and each row gains `relied_notes_listed` (an index line and no content delivery of the note in that turn) and `index_lines`; `per_turn_fts_hits` counts `recall` and `pinned` only (since rev 3.15 it had counted index lines). Totals gain `rows_with_every_relied_note_listed_or_delivered` beside `rows_with_every_relied_note_delivered`.
  - **`tool` still counts as delivered, for the owner's eye.** Kept as the brief and the archived reports have it, but since rev 3.14/3.15 a `memory_search` or `memory_timeline` index row records `tool` with a title and no body, so `relied_notes_delivered` can count a note whose body never reached the model when the reader searched and did not read. `memory_used` names no tool, but that needs no production change: `eval_stack` registers the read tools itself, so the harness wraps them (a recorder that changes no name, schema or result) and records per turn which notes `memory_read` returned with a body and each `memory_search`/`memory_timeline` call with its `detail` (built in the L8-prep review's fix round). Each row gains `memory_reads`, `memory_listings`, `relied_notes_read` and `relied_notes_read_or_excerpted` (excerpted by the per-turn recall — `recall` or `pinned` — read with `memory_read`, or a `snippets` search's excerpt), and totals `rows_with_every_relied_note_read_or_excerpted`: the retrieval measure that compares the two modes like for like, where the `relied_notes_delivered` family counts a title-only row. The README's recipe says so.
  - **Tool rounds are measured, not read off `reader_llm_calls` (deviation from "reuse").** The main loop publishes one `LlmCallCompleted` per turn with the turn's totals, so `reader_llm_calls` is 1 on every answered row — the archived A and B reports' 470 rows included, whatever their tool calls. Each row gains `reader_rounds`, the router cost tracker's count of the calls booked to the main loop's agent id during the turn (a tool round, the answering round, the answer guard's corrective round); `reader_tokens_in` was already the whole turn's. Totals gain `reader_tokens_in_mean_per_turn`, `reader_tokens_in_median_per_turn`, `reader_rounds_mean_per_turn` and `tool_calls_mean_per_turn`.
  - **compare.py** prints each report's `recall_mode` on its header line beside the judge fields and never refuses a mismatch; the archived reports record none and print none.
  - **Tests, offline:** the refusal; the variable through `Settings::from_env` and `run_system` (a `none` run on the stub reader) and a replay's conditions into `conditions.recall_mode`; and, on a stub reader that can now make tool calls, three relied-on notes in index mode — two excerpts and one listed, counted `listed`, then delivered by a `memory_read` of the listed id in the same turn (2 rounds) — against excerpts mode, where all three are delivered.

- **rev 3.19 (2026-09-25), L10c's re-review fixed: a conversation summary holds no key for learning (R1; consequence of rev 3.18's deviation, for the owner's eye).** A summary is written with no `fact_key`, but a model's `memory_update` edit may set one, and since rev 3.18 the note stays a summary. When no other active note held that key, the summary became its R9 winner: learning named it as the key's target, so the user's own later statement under the key was stored as a candidate suggesting to replace the assistant's record of the conversation instead of as an active note, and the key reached the extractor's `<known_keys>` — the summary entering the extractor's prompt, against §13.3. The index's two learning lookups now skip a summary as its automatic search does (`MemoryIndex::fact_key_winner`, `MemoryIndex::fact_keys`): its key names no target and is not listed, and the user's statement is learned active and replaces nothing. The key stays on the note; refusing a non-user edit that sets a `fact_key` on a summary is a new refusal and, with rev 3.18's alternative, the owner's to choose instead.
- **rev 3.18 (2026-09-25), L10c's review fixed (one commit per finding).** One correction of rev 3.17's reading and one deviation:
  - **A conversation summary is an `episode` of origin `extracted_other` that is not a candidate** (finding 1). Rev 3.17's test was kind and origin alone, and the extractor proposes `extracted_other` episodes too — as candidates, like everything it infers from the assistant's words (MD-2, LR3). Such a candidate was taken for a summary: it counted as one covering the rows it cites, so the conversation's episode note was skipped (`summary_coverage`); learning's re-run guard lost it; forgetting it suppressed none of its sources, against LR12's "forget is unchanged"; and deleting its conversation forgot it. `memory_files::is_conversation_summary` now takes the note's stored `status`, and every caller passes it — the forget mode, the conversation delete, learning's lookups and the eval harness. The index's automatic filter is unchanged: it serves active notes only.
  - **A model tool's edit or archive leaves a summary a summary (deviation; finding 2, for the owner's eye).** A tool call records `extracted_user` for what it writes (03 §4, §7a), so a `memory_update` edit re-recorded a summary as learned from the user's words: automatic recall delivered it on its own (against §13.3's "never injected on its own"), the Memory view labelled it *Learned from what you said*, deleting its conversation no longer forgot it, and forgetting it suppressed the conversation's rows again. An edit or archive of a conversation summary by anyone but the user now keeps origin `extracted_other` (`Partition::update`); a user-directed edit still records `user` and makes it the user's note (G2). The alternative the review named — refusing a tool's edit of a summary with `authority_conflict`, as S10 refuses one of a suggestion — adds a refusal and was not taken; it is the owner's to choose instead. `docs/Daemon_Manual.md` says so.

- **rev 3.17 (2026-09-25), L10c implemented (`52668aca` migration 045, `83e8e469` the key, `df835d96` the note and its rules, `f188ec89` the writer and its triggers, `bc673ad2` the conversation deletes, `c7dc0253` the episode split and the harness, `b093d9bb` the wording, the documentation commit `32621ebe` that records this entry, and `f6c700bf`, the crash re-run's advance; 03 is not folded — its §4/§6 forget contract, §5's automatic filter and §7a's list of writers are left for the lead; CLAUDE.md's schema head, 45, likewise).** Readings and deviations taken while building §13.3, each recorded here rather than silently:
  - **A migration, 045 (deviation from §8.9's "one migration").** "At least 4 user turns … since the session's last episode" needs a durable watermark apart from the learning cursor — a summary waits 30 minutes, learning two, over the same rows — so `learning_cursor` gains `episode_through`, seeded at every existing session's newest message (historic conversations are not summarised, as they are not re-extracted). Deriving it from the episode notes instead would have re-summarised a window after its note was forgotten — the forgotten text back in new words.
  - **Automatic recall excludes a *conversation summary* — `kind = episode` and origin `extracted_other` — not every `episode` (deviation from the literal `kind = episode`).** *(Corrected in rev 3.18: and not a candidate — an `extracted_other` episode the extractor proposed is a candidate like any other.)* §13.3's reason ("an episode is a record of a conversation, not a claim about the user", the exception to MD-2) is about the assistant-written summaries; an episode the user saved (`/remember --kind episode`) or one learned from their own words is an ordinary note, and the gold set's six episode notes, relied on by held-out questions, stay recallable, so the D run is unchanged. The filter is the index's automatic mode (per-turn recall, the index block and the lead's opening recall alike). A **pinned** summary is still delivered: the pin is the user asking. Learning's known-notes lookup never shows one to the extractor either — the extractor's prompt is a prompt (`memory_files::is_conversation_summary`).
  - **Forgetting a summary suppresses no source (deviation from "work as for any note").** A summary cites the conversation's own rows, so a forget's suppression would turn every note those rows taught `source_forgotten` — forgetting a summary would silently take the conversation's facts out of recall. Its tombstone is a withdrawal's (LR12's shape); the verbs are unchanged. A fact forget still marks a summary citing the same row `source_forgotten`, and a turn a forget suppressed is left out of every later summary (its text never reaches the model).
  - **Sources: at most eight** (03 §7a's cap, which the file layer also enforces on every later edit): the window's rows when they fit, else eight spread from the first to the last. Deleting a conversation finds its summaries through them.
  - **A crash re-run.** A window an episode note already covers — its write landed, its cursor's advance did not — is passed with no call and no second note, and only as far as that note's newest cited row, which is its window's last (`summary_coverage`): turns that arrived after the crash are summarised in their own window.
  - **The window.** The four turns are counted over every unsummarised row up to a turn in flight; the note covers the next chunk that fits learning's byte budget and never mixes roots (§1.6), with no context rows. One call per note; an explicit flush and the drain write one per remaining window of four or more turns, an automatic one at most four. The note: `Conversation on <YYYY-MM-DD>: <topic>` (the last turn's date; the topic cut to 60 characters), `## Knowledge` = `Discussed:` / `Decided:` / `Left open:` lines cut to 1 KiB, `observed_at` the last row's time, `ExtractionProvenance::Summary` (refused for any other kind, a `fact_key`, a `supersedes` or a replace policy); the prompt is a committed fixture with a golden (`episode_prompt.txt`).
  - **The triggers.** The 30-minute quiet period is the sweeper's; the switch is the sweeper's archived check and the gateway hook's lane switch; the drain writes the summaries after every session's facts. **A boot catch-up is added** (reading: §1.5 makes it the stop drain's durable half, and a drain rarely finishes on a local model): a session with four of the user's turns past its episode cursor is due at boot. A session's facts are learned before its summary; neither the 120-second idle nor the ten turns triggers one.
  - **The gates.** Summary calls are billed as `orchestrator_episode`; learning's own gate now sums both labels, so the two share `extract_max_calls_per_day` and `extract_max_daily_cost_usd`; `episode_max_per_day` counts summary calls. The key sits in `[orchestrator.learning]` (registry `daemon.orchestrator.learning.episode_max_per_day`, 0..=1000) and **`0` writes none** — a literal cap of zero, and the summaries' own off switch — where `extract_max_calls_per_day = 0` is no cap. An unusable answer holds the window with learning's backoff; the third in a row passes it, counted in memory, not durably.
  - **The conversation deletes** are LQ4's three (§1.5): `DELETE /v1/sessions/{id}`, `DELETE /v1/chat/history` and the workspace purge, each reading the conversation's message ids before the rows go (`ConversationRepository::message_ids`, `PurgeOutcome.message_ids`) and forgetting, on the user's surface and best effort, the summaries whose sources all lie among them. The purge plan's `memory/` line stays a `keep` and says so.
  - **The harness and the split.** Only L writes summaries (at each session's `switch`, after its facts — the one deterministic stand-in for the three triggers); L-S and the replays do not. Every fact measure leaves them out (`learning.learned`, the coverage search) and they are dumped as `learning.episodes`. `questions-episodes.json` has 20 questions (en 13, zh 5, mixed 2), one per conversation of at least four turns, each drawing on a noise or rejection turn — what no gold note states; its rows are the held-out schema plus `session` and `turns`, so an L run answers it through `OPENALPACA_MEMORY_EVAL_QUESTIONS`. `Category::Episodes` is not in `Category::ALL`.
  - **The name.** The daemon's "conversation summary" is already the compactor's job (`summary_max_daily_cost_usd`), so what a person reads — logs, the manuals, `daemon.toml`, the registry — says **episode notes**; the code keeps §13.3's name.

- **rev 3.16 (2026-09-25), L10b's review fixed (one commit per finding).** One deviation from rev 3.15's reading:
  - **A skill that cannot call `memory_read` is recalled in `excerpts`** (finding 1). Rev 3.15 had every automatic recall read the key, but the skill tier offers a file-based skill exactly the tools its frontmatter resolves (`orchestrator/skill/invocation.rs`: `requires_capabilities` when it names any, else `tools.allow`, less `tools.deny`), and all four shipped skills name only `file_read` or `shell_execute`. Under the default `index` a `/explain-code` turn got 2 excerpts where it had up to 8, plus up to 8 lines under a header telling it to "read with memory_read", which it cannot call. `MemorySource` now recalls a skill-path turn in `excerpts` unless its skill can call `memory_read` (`skill_can_read_memory`, mirroring those branches; no skill document counts as cannot); a skill that can follows the key. §13.2's line "read with memory_read" stays true wherever the index block is sent. The main loop and the lead are unchanged. `docs/Daemon_Manual.md` says so.

- **rev 3.15 (2026-09-25), L10b implemented (`19f46c61` the golden of today's delivery, characterized first, `d43edb35` the key, `c812dfbb` index-first recall, `8acf0394` the GUI's label, and the documentation commit that records this entry; 03 is not folded — its §5 step 8, §7a `memory_used` enum and the recently-used row are left for the lead).** Readings and deviations taken while building §13.2, each recorded here rather than silently:
  - **An index line carries the full id, not `<id8>` (deviation).** `memory_read` takes hyphenated UUIDs only (L10a, `tools/builtins/memory_tools.rs`), so an 8-character prefix could not be read "with `memory_read`", as the header line tells the model to; the excerpt headers already carry full ids. The line is otherwise §13.2's: `<id> <date> <kind> “<title>” — <reason>`, where `<date>` is the index row's `date` (`observed_at`, else `created_at`, canon RFC 3339) and `<reason>` the match reasons as an index row joins them (`-` for a short query's substring hit). A title's line breaks become spaces, so a line stays one line.
  - **The counts.** `max_items` (8) still bounds the delivered items — pinned notes and excerpts; the index block's 8 lines are counted apart, inside the same 8 KiB. The header line is paid for by the first listed note, and there is no header when nothing is listed. With two recalled notes or fewer the two modes deliver the same bytes.
  - **A note too large to excerpt is listed instead.** §13.2 is silent on a top-2 note the budget cannot hold as an excerpt; it is listed (when a line fits) and the next note may take its excerpt place, rather than dropped as the excerpts delivery drops it. Listed notes are re-read and re-hashed like excerpts (§5 step 7): the title and date on a line are the file's, never the cache's.
  - **Where the mode applies.** `RecallBudget` gains `excerpts` and `listed`; `RecallBudget::for_mode` builds each mode's budget and `Default` stays design §4's, the excerpts delivery. Every automatic recall reads the key: the main loop and the skill tier through `MemorySource`, which reads the live config on every turn, and the lead's opening recall (03 §5's one read path). *(Corrected in rev 3.16: a skill turn whose skill cannot call `memory_read` recalls in `excerpts`.)* The trust label is unchanged, so the excerpts delivery is byte for byte today's (`prompt_ctx/sources/memory_recall_excerpts.golden.txt`, written against the unchanged code in its own commit).
  - **Order and room.** `memory_used` names the pinned notes, then the excerpts, then the listed notes; a `<memory_suggestions>` block (§4.5) takes its room from the listed notes first, and the header goes with the last of them.
  - **A tool's listing stays `tool`.** Rev 3.14 left to L10b whether a `memory_search`/`memory_timeline` index row should move to `index`: it does not. `index` is the automatic recall's reason, as §13.2 defines it; a row the model's own search returned is what §7a's `tool` names.
  - **The key.** Registry `daemon.orchestrator.recall_mode` (the `[orchestrator.memory]` table's convention: its other keys are `daemon.orchestrator.<field>`), `Enum(index, excerpts)`, default `index`; the CLI maps it to `[orchestrator.memory] recall_mode`. It is serde-typed, so a value the key does not have fails the file like any other mistyped key: the daemon starts on every default with its one `WARN`. `scripts/release/templates/config/daemon.toml` has no `[orchestrator.memory]` table and gains nothing, as L1's keys did not.
  - **Beyond §13.4's files:** the GUI's *Notes used in this answer* reads a listed note as `— listed`, the section's word, where it printed the bare reason (`MemoryUsedNote.tsx`, `chat-stream.ts`'s union).
  - **For L8.** The harness (`tests/memory_eval.rs`) is not changed: its D run now recalls in `index` mode by default (`eval_daemon_config` starts from `DaemonConfig::default()`), its report's conditions do not name the mode, and its `per_turn_fts_hits` and `relied_notes_delivered` count a listed note (any reason but `tool`, any reason) as delivered. L8's `D-excerpts`/`D-index` runs should set the mode, record it, and decide whether a listed note counts as delivered. (superseded: rev 3.20)

- **rev 3.14 (2026-09-25), L10a implemented (`b71d6e9d` the index, the tools and the routes, `3ed33b2a` the CLI, and the documentation commit that records this entry; 03 is not folded — its §7 tool table, §7a `memory_search`/`memory_read` contracts, route table and CLI syntax are left for the lead).** Readings and deviations taken while building §13.1, each recorded here rather than silently:
  - **The index carries dates (schema 3).** A search that filters and orders by date before its limit needs the date in the catalog, so `note` gains `created_at`, `observed_at` and `date` (= `observed_at`, else `created_at`, canon RFC 3339) and an index on `(partition_key, date, id)`. The index is a disposable cache: a v2 file is recreated at open and reports `rebuilding` until the rebuild. No database migration.
  - **An index row keeps `conflict`.** §13.1 names eight fields; the row also carries §7b P3's `conflict` flag, because the index is now what a search answers by default and §7b P4 says a same-key loser is returned flagged. `date` is the full canon timestamp; `reason` is the match reasons joined by `,` (`-` for a one- or two-character query's substring hit, §7b P5), and on a timeline `before`, `anchor` or `after`; `status` is `active`, `superseded`, `archived` or `ended` (active, window ended at `as_of` or the index clock's now); `size` is the body's bytes ÷ 4, rounded up.
  - **Date bounds.** `date_from`/`date_to` (and a timeline's `date`) take RFC 3339 or `YYYY-MM-DD`, a whole UTC day — a lower bound or an instant from its first second, an upper bound to its last — so an inclusive `date_to` keeps the day it names. `date_from` after `date_to` is `invalid_input`. `newest`/`oldest` keep relevance order among notes of one date.
  - **Limits.** The tool's and the route's `limit` is 1–50, default 10. The automatic recall and learning's lookup still ask the index for 20 hits (`RECALL_CANDIDATES`, `LOOKUP_CANDIDATES`), so neither a turn's delivery nor what the extractor is shown moves with the tool's cap; the index's own `SearchQuery::new` default stays 8.
  - **`memory_read` with ids.** 1–10 distinct ids (a repeat is `invalid_input`); `expected_revision` and `section` are kept but only with a single id. One entry per id in the order asked: the §7a read object, or `{id, error: "not_found"}` / `{id, error: "revision_conflict", current_revision}` in its place — an absent id is no longer a refusal of the call. The 32 KiB is split evenly in the max-min sense: a body shorter than its equal share keeps all of it and the rest goes to the others, so a short note is never cut to make room and the shares never sum past 32 KiB; a single-id read keeps the whole budget. The old `id` argument is `invalid_input`.
  - **"A foreign id … without leaking the others' order"** is read as: another owner's id answers exactly as an id no note ever had — `{id, error: "not_found"}` in its own place, byte for byte the same answer — and the other notes come back unchanged in content and order; the foreign note is neither read nor recorded.
  - **`memory_timeline`.** A `query` anchor is the first hit of a *historical* search (so a superseded or archived note can anchor, as it can be listed); a query nothing matches draws an empty timeline with `anchor: null`, not an error. A note anchor that is a candidate is `invalid_input`; one the owner does not have is `not_found`. A `date` anchor lists notes dated before it, then at or after it. `before`/`after` are 0–20. The timeline covers every partition the context reads (no scope or kind filter); each row is re-read from its file before it is served, with one re-run when a re-read finds the index out of date, as a search does.
  - **`memory_used`.** A search hit is recorded as `tool` whatever its detail, and so is a timeline row — §7a's `tool` is "a note the model fetched through `memory_search`", and an index row puts the note's title in front of the model; the timeline's own anchor search records nothing. L10b's `index` reason is recall's; whether a tool's listing should move to it is L10b's call.
  - **The lead's surface.** The lead's hand-built tool list gains `memory_read` and `memory_timeline` beside `memory_search` (all three the `memory_read` capability's): the search now answers an index, and without the read the lead could not open what it found. Subagents get the timeline through the capability.
  - **The one-line guidance** did not exist before: it is a `<memory_rules>` system block, one line, added only on a main-loop turn whose surface carries `memory_search`, beside `send_rules`.
  - **Route and CLI.** `GET /v1/memories/search` takes `detail` defaulting to `snippets`, so the Memory view's call answers as before (its default page grows from 8 to 10); `GET /v1/memories/timeline` takes `anchor`, `q` (as the search route names its query), `date`, `before`, `after`. `openalpaca memory search` prints snippets and sends `--since`/`--until`/`--order` as `date_from`/`date_to`/`order_by`, `--limit` default 10; `openalpaca memory timeline` takes an id (a unique prefix will do), `--query` or `--date`, and `--before`/`--after` (default 5).
- **rev 3.13 (2026-09-25), L9 implemented (`15318ddd` the commands, `3888ab04` a test that no longer depends on tie order, `a1e93d70` the reserved names, `a295df2e` the REPL, and the documentation commit that records this entry; 03 is folded as its rev 6).** Deviations and readings taken while building it, each recorded here rather than silently:
  - **A read-only lookup in the service.** §12.4 names one new service item, the constructor. `/forget <id-or-prefix>` also needs every note a request can read by id prefix, and paging `list` from the orchestrator — as the CLI pages the route — reads every note once per page; `MemoryService::find_by_id_prefix` reads each partition once. It resolves among **every status** (superseded, archived and candidates too), as `openalpaca memory` resolves a prefix, ignores case and hyphens on both sides, needs `memory_read`, and records nothing. `ID_PREFIX_MIN_HEX` is 8.
  - **`gui` and `cli`.** §12.2 names both lane sources and the code accepts both (`ATTENDED_CHAT_SOURCES`). L4's `memory_confirm` checks `gui` alone; the two coincide today, because the CLI's chat is `EventSource::Gui` and no production path derives `cli`.
  - **The order of the refusals.** The attended-chat check comes before the owner check, in the handler and again in `context_for_chat_command` (which refuses any other turn `invalid_input`, so a `UserSurface` context cannot be built from one). So `/v1/command process` — source `api`, `Principal::System` — answers §12.2's first sentence, as its first bullet lists it, and `Principal::System` on an app turn the second. An unlinked `External` principal never reaches the handler: the gate refuses the turn (`chat.respond`) first. An **unattended** app or CLI turn (a piped `openalpaca chat`) answers the first sentence too — §12.2 names no sentence of its own for it; the CLI manual says why a pipe gets it.
  - **Every answer is `Ok`.** A refusal is the turn's answer, not an error, so `done.is_error` stays false and `/v1/command process` does not report it `rejected`.
  - **Answers §12.1 does not fix.** An exact duplicate: `Already saved: “<title>” (<id8>, <scope>). Nothing new was written.` A refusal: `Nothing was saved:` / `Nothing was forgotten:` / `Nothing was searched:` / `Your notes could not be listed:` before the service's own sentence. `/forget` with no argument says how to use it and forgets nothing; a prefix nothing has: ``No note of yours has an id starting with `<prefix>`. Nothing was forgotten.``; a revision that moved between the lookup and the forget forgets nothing and says to send it again. `/memories` with no notes: `Nothing remembered yet. Send /remember <text> to save a note.`; with more than ten: `More notes are kept: send /memories <words> to search them.`; one suggestion reads `1 suggestion is waiting…`, and the count is the suggestions the Memory view lists (one per note, `older_count` aside), `At least 100` at the route's cap.
  - **An ambiguous prefix lists full ids**, not `<id8>`: notes sharing an 8-character prefix share their `<id8>` too, so the short form could not tell them apart. A note that is not in force carries its effective status in the tag (`[fact/global/archived]`).
  - **The options.** `--kind=<k>` is accepted beside `--kind <k>`, the kind in any case; `--` ends the options, so a text may start with dashes; an unknown `--option`, a repeated option and both scopes are refused `invalid_input` rather than saved as text.
  - **Reserved names, both ways.** The loader's one `WARN` covers a plugin-contributed skill as well as a SKILL.md one, and every name that reaches a skill by slash — its slash command, an alias, and its id (the `/{skill_id}` fallback). `create-skill` refuses the names in its instructions and in `scaffold.sh` and `write_skill_md.sh`, which exit non-zero before creating anything.
  - **Beyond §12.4's files:** the REPL's `/help` and Tab completion list the three commands (`a295df2e`); they were forwarded already. `/forget` raises no confirmation card: typing the id is the say-so, as on `DELETE /v1/memories/{id}`.
- **rev 3.12 (2026-09-25), L7's re-review fixed (`2f685ca3`).** Rev 3.11's `forced_rerun_checked` — "the flush made at least one call" — still passed a re-run that examined nothing: `FlushReport.calls` counts a call before its answer is parsed, so a first unusable answer (`stopped = BadOutput`) or a later chunk that found no model (`Unavailable`) left the forgotten row's chunk unread and read as checked. The rule is now a call **and** a flush that read its session to the end (`stopped` is `null`); the re-run's report line prints the stop, and the README's `learning.invariants` row says the same. No deviation from §7.3: its invariant was already hard.
- **rev 3.11 (2026-09-25), L7's review fixed (`bfa78da3` finding 1, `b5c942e7` finding 2, `123c468f` finding 3).** What the fixes add to what §7 and rev 3.10 say:
  - **A forced re-run that checked nothing is a violation** (finding 1). `forgotten_resurrected` counted what a re-run brought back, so a re-run that was skipped (no candidate note, the forget failed, the cursor could not be reset) or whose flush made no extraction call reported 0 and passed. `invariants` gains `forced_rerun_checked` (a note was forgotten and the flush made at least one call); for L and a replay — where §7.2 requires the re-run — an unchecked one, or none, sets `violated`, which fails the run after the report is written like §7.3's two invariants. L0 and L-S run no re-run and are not held to it.
  - **L-S seeds four old notes as the user saved them then** (finding 2), a deviation from §7.2's "the 8 *old* notes … seeded … through the existing seeding path" read as the gold text. Four gold old notes (`user.editor`, `user.laptop`, `user.commute`, `moss.output_dir`) record their own replacement — "Until June 2026 … Replaced by Helix", "…2026 年 4 月 20 日卖掉了", a `valid_to` — and §2.5's change test drops a change item whose listed saved target it does not name by `replaces` as `unchanged`, so a correct extractor could raise no suggestion for those pairs and the "8 suggestions" would have measured the seeding. The harness's `LS_SEEDED_AS_SAVED_THEN` gives those four a present-tense title and body and no `valid_to`; id, kind, scope, key, aliases and tags stay the gold note's, and the report lists the four in `conditions.learning.seeded_as_saved_then`. D and E seed the gold set unchanged. This closes rev 3.10's "for L8 to read before the run" item.
  - **The live learning path has offline tests** (finding 3): the gateway feed through `Learner::feed` as `run_system` wires it, with its flush cadence, turn-to-row map and bus counts, the forced re-run, and L-S's `suggestion_check` with the user-directed confirm, all on the stub model. For them the orchestrator set-up moved out of `run_system` into `eval_stack` and the gateway into `Learner::new`, unchanged, and the stub router lifts its rate limits: at the router's default 40 000 TPM a fed turn's reader prompt waited ~6.7 s for the bucket. **For L8:** the same default applies to the live run's reader when the scratch `llm.toml` sets no `[rate_limits]` (the README's example sets none), about 1.5 s per 1 000 estimated prompt tokens once the bucket is spent; the time estimates in §7.5 do not include it.
- **rev 3.10 (2026-09-25), L7 implemented (`6b1f1d5d` the corpus and its test, `b881b527` the harness).** Deviations taken while building it, each recorded here rather than silently:
  - **The corpus** (`conversations-learning.json`): 51 sessions, 299 user turns, 32 English / 14 Chinese / 5 mixed sessions, 26 with no project / 18 Lantern / 7 Moss; all 83 gold notes stated (every one is relied on by a held-out question), 23 only indirectly; the 8 pairs old-then-new, with three `user.*` pairs (laptop, editor, weekly review) changing in a Lantern session; 7 corrections; 14 restatements, 6 of the pairs' old notes; 11 rejections; 55 % noise; 13 suggestion sessions. A rejection is a user turn that turns down what an earlier turn in the session solicited, since the corpus carries only user turns and the reader's proposal is live. The validation adds checks beyond §7.1's list: story order, no `/` turn, noise exactly when `states` is empty, a project's notes only in that project's sessions (so they learn into the project partition, LR1), a `change` turn states only pairs' new notes, a restatement repeats an earlier session's fact, and the suggestion sessions carry every change and old-note restatement. Normalisation is the memory index's own `normalize` (NFKC, lower case, Latin diacritics dropped): one step stricter than §7.1's "NFKC and case-folded", and no new dev-dependency.
  - **`tests/common/eval.rs` is included by path** (`#[path = "common/eval.rs"] mod eval;`) in both test files, with no `common/mod.rs`. It holds the data types, the corpus types, the change-pair derivation (pairs are derived from the gold set by scope, project and `fact_key`, not listed in the corpus) and `is_cjk`.
  - **The coverage judge names the note.** §7.3 has it answer "whether any of them states the gold knowledge", but *noise* needs to know which one it chose, so the frozen prompt ends `MATCH <n>` or `NONE`. The search is historical with limit 5; candidates are not searched, so a suggestion never counts as coverage. A gold note whose search returns nothing costs no call (`not_found`).
  - **Corrections are judged on the gold value only**: `new_active`, `new_not_active` or `new_missing`. Whether the wrong value was also written is not measured; that would need a judge call per correction with the wrong value named in the corpus.
  - **Restatement outcomes gain `first_learned`** (an active note citing the turn with no other note of its key or title: the earlier statement was never learned, so it is no duplicate) and `candidate`.
  - **The extraction's daily caps are lifted** for the learning systems (`extract_max_calls_per_day = 0`, `extract_max_daily_cost_usd = 1e6`), and recorded: a binding cap would stop a manual flush part-way and leave the phase under-learned.
  - **The forced re-run runs after the questions**, not right after the learning phase (§7.2 "inside L"), so the forgotten note cannot change what the questions or the coverage read. Resurrection is defined by provenance: a note the re-run wrote citing a row the forget suppressed. A note with the forgotten one's title or key citing other rows is recorded apart and is not a violation.
  - **The replay** copies the kept database into a new tempdir, rebases it onto a fresh owner and a fresh scratch project (so both partitions start empty and the kept scratch is only read), deletes every cursor row and flushes each stored session whole, in order — so its chunking is not the live run's 10-turn one. An L run writes `learning-transcript.json` (session and turn-to-row map) beside its kept database for it; L always keeps its scratch.
  - **Re-judge** keeps the archived report's system and reader figures, adds `original_judge_verdict` / `original_judge_model` per row and `agreement_with_original` (raw and Cohen's κ) to the totals — the numbers §7.5 step 1 asks for — and records `rejudged_from` with its SHA-256. Its judge is the eval's default model unless named, recorded against the archived report's reader.
  - **The guard's post-call check runs on every judge call**: the first mismatch stops the run as §7.4 says; a later one fails its own row (`judge_error`). "Every row's `reader_models` must name the reader alone" is asserted after the final report is written, as are the invariants.
  - **Two variables beyond §7**: `OPENALPACA_MEMORY_EVAL_JUDGE_MODEL=none` makes no judge call (every row `not_judged`), because the routability guard would otherwise make the README's no-provider dry run impossible; `OPENALPACA_MEMORY_EVAL_SESSIONS=N` feeds the first N sessions, a smoke run of the L pipeline. Both are recorded.
  - **`conditions.extraction_disabled` stays**, derived from the run (true when no learning ran), beside the new `conditions.learning`: the schema is added to, never changed.
  - **L0 is measured like L** (the dump, coverage, restatements, aliases, keys) on what the reader's own writes kept: 83 coverage calls, for the L-against-L0 reading.
  - **`compare.py`** compares the judge models of the paired rows; a report that records none against one that does is refused too, and differing `judge_prompt_sha256` values only warn.
  - **The two core tests** (`flush_session_runs_to_the_last_row_without_the_quiet_rule`, `a_manual_queue_never_flushes_on_its_own`) pin behaviour L1 built; they pass at HEAD and are shown to fail with `flush_session` on the automatic policy and with `try_start`'s mode check removed. The harness's own tests run on an offline stub model registered as `qwen3.8:27b-mlx`: nothing reaches `localhost:11434`.
  - **For L8 to read before the run:** L-S seeds the old notes through the existing path with their gold bodies and validity windows, and four of those bodies already name their replacement ("Replaced by Helix in June 2026"), which may lead the extractor to read a change turn as a restatement. The design says to seed them as they are, so they are.
  - **Left for the lead** (outside this slice's writable documents): CLAUDE.md is untouched; §11 has no L7 row; §8.11 assigns L7 no manual, and the README is the runbook. `docs/api/` did not change.
- **rev 3.9 (2026-09-25), L6 implemented (`9ade5905` the characterization, `5d03c39e` the quick wins, `f93c1d29` the parse and snapshot caches with the benches).** Deviations taken while building it, each recorded here rather than silently:
  - **The characterization is a commit of its own**, before §8.12's two, as L1–L5 did. It pins at HEAD what the quick wins change — a store read the partition 3 times, an edit 5, a service forget 4 per target, a partition forget of twenty notes 40, a store built an index record for every note, an open with twenty tombstones scanned `notes/` 20 times — and the quick-wins commit flips each (1, 2, 2 per target, 1, one record, 1). `reconcile_outputs_are_unchanged_on_a_10k_partition` stays green across it.
  - **Quick win 5 was already in the tree:** L2's lookup takes its keys from the index (`MemoryIndex::fact_keys`). `learning_view_parses_no_partition` pins it.
  - **A sixth win of the same kind:** recovery at every open listed `notes/` once per tombstone to look for leftovers, which is O(tombstones × notes) at each open, and every forget adds a tombstone. It lists `notes/` once.
  - **§8's "a 20-target `forget` 1 load" is the partition's `forget`.** The service's `forget` still finds and forgets each target as its own mutation, so a failure part-way leaves every earlier target published and indexed, as before. Each target costs two reads (find, then check) where it cost four.
  - **Quick win 3's API.** `Partition::{create,update,forget,reconcile}_viewed` return the partition as a fresh read would find it. `LoadedPartition` gains `scan`, the stat scan it was read from; after a write it carries the written file's own stat, and the service's re-index stamps it instead of scanning again. The written bytes are classified exactly as a read would classify them (the one note is parsed back), not taken from the in-memory `Note`, so the view is byte-for-byte a fresh read. `the_view_a_mutation_returns_is_a_fresh_read` compares the two after every kind of write. `forget` rebuilds `MEMORY.md` once, after the last target, and also when a target fails, for the targets before it.
  - **The stat identity adds `nlink`** to `(mtime_ns, size, inode, ctime)`. A note file that gains a hard link, which the read refuses, is then read again whatever its timestamps say.
  - **A racy window, not in §6.3** (git's "racily clean" rule). Linux file timestamps advance once per scheduler tick (up to 10 ms), so a same-size in-place edit inside the tick after a read keeps the file's whole stat identity. The cache would then serve the old parse to every *read* until the file changed again; mutations are safe either way, by the invariant. So an entry whose stat was taken within `RACY_WINDOW` (100 ms, or 3 s on a file system with whole-second timestamps) of the file's last change is re-read and re-hashed at every read until it settles, and its parse is reused while the bytes still hash to it. A write of this process goes in untrusted, so the next read re-hashes the one file it wrote and parses nothing. `NoteParseCache::with_racy_window` is the test knob.
  - **The stat function** is `NoteParseCache::with_stat(impl Fn(&Path) -> Option<StatKey>)`. It is a closure, so the invariant test can hold the frozen key, and it returns `Option`, so a file that cannot be stat'ed is not cached. The default takes the partition scan's own `lstat` rather than a second one per file.
  - **The invariant covers every revision check,** not only the file a mutation changes. `create`'s `supersedes` target and a confirm's target (S5) are re-read and re-hashed too, since both bind a revision. A view whose bytes are not the file's is evicted and the partition read again, so both the check and the note the write starts from are the disk's. `a_same_stat_external_edit_is_never_overwritten` exercises `update` and `forget`; with the re-hash removed it fails with "a write over an unseen edit: Ok(())".
  - **The snapshot cache.** The generation is one process-wide counter, bumped after every `ControlSnapshot::save`, so a save through a handle without the cache retires every cached snapshot. A save through a cached handle records its own snapshot once, after the mutation's last save, under the generation that save wrote, and only if no other save came in between. The partition holds its snapshot as `Arc<ControlSnapshot>` and unshares it (`Arc::make_mut`) only to change it. Recovery and reconciliation read before they unshare, so an open with nothing to recover copies nothing. Snapshots are not counted against the 128 MiB, since there is one per partition the service opened. Capacity 0 caches nothing, snapshots included, so the benches' *cache off* is the uncached path.
  - **An index rebuild clears the cache at its start,** not at its end where it clears the fingerprints: clearing there would empty what the rebuild had just read.
  - **A file that is no note is never cached.** It is read at every read, as before, and its presence does not make each read rewrite the partition's entries.
  - **The benches.** `memory_index/bench.rs` has one body, `run_bench(name, n, mix)`, behind `bench_search_10k_notes_with_verification`, `bench_search_100k_notes_with_verification` and `bench_search_100k_learned_notes_with_verification`. The average mix is the slice-2 corpus draw for draw: a throwaway test hashed both generators' 10 000 records and 200 queries, and the digests were equal. The learned mix aims its learned bodies at 200–300 B plus one sentence, so 70 % of its notes land at 400 B or less; `the_bench_corpus_mixes_have_their_shapes` pins both mixes in default CI. `memory/service/bench.rs` has `bench_service_{1k,10k,100k}_notes` and `bench_service_100k_learned_notes`. It seeds by writing the files and building the snapshot in one pass with the origins set (45 % `user`, 45 % `extracted_user`, 10 % `external`, one suggestion in a thousand and at least five). It does not call `repair` itself, which records every note `external` and would leave *recently learned* and `pending_suggestions` nothing to list. `the_service_bench_seeds_a_partition_the_service_reads` runs in default CI.
  - **The counters.** `whole_parses_on_this_thread` still counts whole-partition reads, whether their files came from the cache or the disk. `note_file_reads_on_this_thread`, `scans_on_this_thread` and `snapshot_loads_on_this_thread` join it, and core's `#[cfg(test)]` `RECORDS_BUILT` counts index records built.
  - **Tests beyond §8's list:** `the_view_a_mutation_returns_is_a_fresh_read`, `opening_a_partition_scans_its_notes_once`, `a_file_changed_within_the_racy_window_is_re_hashed_until_it_settles`, `a_cached_snapshot_is_read_again_after_a_save_or_a_replacement` and `the_service_reads_an_unchanged_partition_through_its_cache`.
  - **Measured, for L8 (release, this machine).** The 10K bench after the caches: search + verification p50 5.26 ms, p95 10.93 ms, max 16.02 ms; 1 280 B average; cache 107.8 MiB, of which `fts_tri_data` is 52.4 MiB and `chunk` 34.4 MiB. The 10K service bench with the cache on and off: recall p95 58 / 85 ms, `list` p50 33 / 282 ms, learned `store` p95 107 / 378 ms, `update` p95 133 / 638 ms, `forget` p95 227 / 653 ms. What remains of a store with the cache on is work the cache does not touch, not profiled here: two full snapshot saves (2 MiB of JSON at 10K), the `MEMORY.md` rewrite and the index update. A fresh service's first reconciliation tick re-indexes every partition whole (3.6 s at 10K), because its fingerprints start empty.
  - **`docs/api/` did not change** (`gen_api_docs.py --check` passes). §8.11 assigns L6 no document, and §11 has no L6 row.
- **rev 3.8 (2026-09-24), L5 implemented (`5942c12f` the characterization, `eb98b820` the templates and the ambient grant, `4d5a13dc` the header and search validity with the subagent ring test).** Deviations taken while building it, each recorded here rather than silently:
  - **The characterization is a commit of its own**, before §8.12's two, as L1–L4 did: `writing_research_and_system_lack_memory_read` flips to `all_nine_templates_name_memory_read` in the grant commit, and `an_upcoming_note_renders_no_window` to `timing_upcoming_uses_the_index_clock` in the header commit, on the same pinned clock (the index clock months before the wall clock, so only the index can call the window upcoming).
  - **A collision §8.10 did not list:** `agent/registry/tests.rs`'s `test_template_with_markdown` counted a spawned instance's capabilities as three (one listed, two workspace tools); the ambient grant is the fourth. The count and a fourth name assertion moved in the grant commit.
  - **The grant's two checks are case-insensitive** (`memory_read` listed or denied in any case), because `AgentConstraints::normalize` lowercases the lists the gate reads, so a template denying `Memory_Read` is denied the tools and must not be granted them either. The appended capability's category is `ambient` (nothing reads the category).
  - **`MemoryIndex::get` also fills `IndexedNote.upcoming`** on the index clock: it returns the same row type as `list_pinned` (§6.1 names the pinned row). Only tests call `get`.
  - **`timing=upcoming` is printed only after the bounds, and only when the re-read file still has a `valid_from`.** §6.1 has the bounds from the re-read note and `timing` from the index; an index row can lag an external edit that removed the window, and the header must not say "upcoming" about a note with no window. `an_unwindowed_header_is_byte_identical` pins both halves.
  - **The ring test runs through the ambient grant.** `a_subagents_memory_search_records_its_task_id_in_the_ring` spawns a worker whose template names no memory capability and gives the lead no memory service (so nothing the lead recalls reaches the ring); the ring's one row is the worker's `memory_search`, reason `tool`, the run's `task_id` and no lane. It fails with either the grant or the service's `task_id` attribution reverted.
  - **Two tests beyond §8's list:** `an_internal_template_is_granted_memory_read_ambiently` (the grant itself, and no second copy for a template that names it) and the storage crate's `hits_and_pinned_rows_say_upcoming_on_the_index_clock` (hits, pinned rows and `get`, and `as_of` moving the instant).
  - **`docs/api/` did not change**: the generator documents none of the items this slice touched (`gen_api_docs.py --check` passes).
  - **Left for the lead** (outside this slice's writable documents): 03's §11 rows for L5 — §7 *Capabilities* line 359, §5 step 8's header and §7a's `memory_search` result — are not folded into 03 yet. §8.11 assigns L5 no manual, so `docs/Daemon_Manual.md` (the memory-tools paragraph could name the ambient grant; the search route row could list `valid_from`/`valid_to`) and `apps/openalpaca-gui/API_MAP.md` (the Memory view's search row) are unchanged; neither is false as written. The GUI's `SearchResult` type does not declare the two new fields, which nothing there reads. CLAUDE.md is untouched.

- **rev 3.7 (2026-09-24), L4 implemented (`63b29ba9` the characterization, `0f2dbb3a` the `ToolContext` literals, `d27c034a` the sandbox, the tool and the events, `e99688c7` the prompt block, then the GUI, the CLI and the documents).** Deviations taken while building it, each recorded here rather than silently:
  - **The service half is its own module**, `memory/service/in_chat.rs` (§8 L4 names `mutations.rs`): `confirm_suggestion`, `dismiss_suggestion` (the Deny hook's withdrawal, LQ2), `suggestion_preview`, `pending_suggestions` and rule 5's check share one reading of "a suggestion from the user's own words", and `withdraw` became `pub(super)` for it. The cited-row check is one primary-key read, `ConversationRepository::learning_rows_by_ids`, judged by `learning::is_user_authored_row` (§1.2's rule as one function).
  - **The `raised` set lives on `MemoryService`, not the learning queue** (§4.5, §8 L4 names `queue.rs`). The main loop and the per-request `memory_confirm` both hold the service; neither holds the queue, which only the gateway does, so the queue would have needed a new orchestrator setter and daemon wiring for one set. It is still in memory and per lane, and it holds the note a raised suggestion would replace as well as the suggestion, so a denied or timed-out card's restatement (another suggestion for the same note, which S4's exact dedup does not catch) is not raised again in the conversation either.
  - **A per-call tool always asks**, even when a policy's explicit `require_confirmation_for` does not list it (§4.2 rule 2 skips the cache and `auto_approve`; it did not say what an explicit list that omits the tool means). No shipped policy lists one; the rule makes the per-call guarantee independent of the list.
  - **`<memory_suggestions>` is bounded at 1.5 KiB, not 1 KiB** (§4.5). One line carries two UUIDs and two 64-hex revisions — 331 bytes before its titles — so three lines cannot fit 1 KiB at all; titles are cut at 60 bytes, lines are added while they fit, and the block still counts against the 8 KiB memory message (the lowest-ranked recalled notes give way, and `memory_used` names what stayed).
  - **Who wrote the replaced note decides the words**, as L3's finding 4 did for the Memory view: the preview says "your saved note" / "your note (edited by hand)" / "your imported note" (labelled `Yours:`) or "the note" (`Current:`, for a note learning wrote), and a `<memory_suggestions>` line says `(saved by you)`, `(edited by you)`, `(imported by you)` or `(learned earlier)` rather than §4.5's fixed `(saved by you)`. A preview with no evidence sentence to quote says `From what you said on <date>.` in place of `Why: …`.
  - **`memory_update action: confirm`'s refusal names `memory_confirm` only where it is offered** (a `gui`-source turn; the preview never runs on an unattended one); elsewhere it names the Memory view as before, so the model is never pointed at a tool it does not have.
  - **Two refusal classes are `MemoryError` variants** (`NotASuggestion`, `ApprovalRequired`). The memory routes' exhaustive match maps them to `409 MEMORY_CONFIRM_REFUSED`, which no route can produce; `HumanApproval::new` is `pub(crate)`, so no code outside the core crate can make the fact either.
  - **The "parses no partition" counter** (`memory_files::whole_parses_on_this_thread`, `#[doc(hidden)]`) is a thread-local always compiled into the storage crate, because the core crate's tests link its non-test build; L6's parse counter can replace it.
  - **The characterization test is a commit of its own**, before §8.12's four, as L1–L3 did; it is flipped (and renamed `a_memory_update_confirm_is_refused_before_any_card`) in the sandbox commit.
  - **`docs/api/` did not change**: the generator documents none of the items this slice touched (`gen_api_docs.py --check` passes at every commit).
  - **Left for the lead** (outside this slice's writable documents): 03's §11 rows for L4 — §4's user-directed surface gaining the approved in-chat confirm, §7's tool table and §7a's tool contracts gaining `memory_confirm`, §7's annotations gaining `approval_is_per_call`, the preview hook and `memory_update`'s confirm refused before the card — are not folded into 03 yet; CLAUDE.md is untouched.

- **rev 3.6 (2026-09-24), L3's review fixed (`5774e8cc` finding 2, `4d0994be` finding 3, `257b5e5c` finding 4, `9b3dff27` finding 1).** What the fixes add to what §3 and rev 3.5 say:
  - **S5 also refuses a target that is no longer in force** (finding 2). A supersede link retires its target without changing the target's file, so after a tool call replaced a hand-edited or imported target (which §3.1's last row allows) or the user ran `memory add --supersedes`, the target's revision still matched and the confirm made a second active note for one fact. After the revision check, a target whose derived status is not `active` is `invalid_input` naming its status and pointing at undo; the candidate is left as it is. The listing already hid such a suggestion (rev 3.5); the detail's Replace and the CLI's `confirm` now meet the refusal. Test: `confirm_refuses_a_suggestion_whose_target_was_replaced_another_way` (both ways of replacing).
  - **S6's re-read cannot fail the confirm** (finding 3): `dismiss_others` returns the dismissed ids with no `Result`, and a partition that cannot be re-read dismisses nothing, with one WARN. That is S6 as written ("the confirm stands"); it has no failing-first test, because nothing lies between the confirm's own read and this one in which to make the second fail.
    *(Rev 3.22: this, and `4d0994be`'s subject, cover S6's re-read only — the write's final `reindex_view(…)?` still answered an index error after a landed write, in the confirm and every other mutation, until the residue slice's R6, `7d5174e9`.)*
  - **The Suggestions wording follows who wrote each text** (finding 4, beyond §3.5's copy). The text a suggestion would replace is labelled `Yours` only when the owner wrote it (saved, edited by hand, imported) and `Current` for a note learning wrote; a suggestion whose own text changed after learning wrote it (origin `user`, `external` or `imported`) reads "Edited by you · suggested on <date>" and the like in the app, "edited by you, suggested <date>" in `openalpaca memory suggestions`, instead of "Inferred from the conversation" or the raw origin word.
  - **`docs/Daemon_Manual.md` is updated in this slice** (finding 1), though §8.11 does not assign it to L3: the undo, confirm, suggestions, recently-learned and list rows, the tool paragraph, the learning paragraph and `memory_changed`. The rev 3.5 "Left for the lead" item for 03 §11 stands; this closes nothing there.
- **rev 3.5 (2026-09-24), L3 implemented (`9e533a66` the characterization, `5748ad05` storage and service with fixture 25, `e83f0ee8` the route and the CLI, `c1794c42` the GUI and the documents).** Deviations taken while building it, each recorded here rather than silently:
  - **Undo answers a withdrawal, not a forget** (§3.5, 03 §7a's route table; **for the lead to confirm**). §7a says `POST /v1/memories/{id}/undo` answers `{ "id", "status": "forgotten" }`, and at HEAD it also carried the forget's `retained`, whose note says the cited sources "will not be re-extracted" — false for a withdrawal, which suppresses nothing (LR12). It answers `{ "id", "status": "withdrawn", "restored": [uuid] }`: `restored` names the notes the withdrawn one had retired and that are effective again (empty for a suggestion, which retired nothing), which is what "Undone — “A” is back" and the CLI's "the note it replaced is back" read. The GUI's `MemoryWithdrawn` and the CLI follow. `memory_changed` still says `change: "forgotten"` for a withdrawal and for each S6 dismissal: the note is gone, and §7a's `change` enum has no other word; no event variant changed.
  - **The suggestions item carries more than §3.5 lists.** Besides `id, revision, title, reason, created_at, origin, scope, sources, target{…}`, each item has `knowledge` (B's `## Knowledge` text — §3.5's GUI row shows "B's knowledge", which the listed fields did not carry), `observed_at` (the newest cited row's time, for "From what you said on <date>") and `older_count`. `reason` is read from the note's own `Replaces “…” (…): <reason>` evidence sentence (§2.6), `null` when there is none. A suggestion whose target is gone or no longer effectively active is **not listed** — nothing is left to replace — and stays a candidate in the list, where undo still dismisses it.
  - **A candidate naming more than one note is not a suggestion.** S5 binds one `target_revision`, so `confirm` of a candidate whose `supersedes` lists several notes (only a hand edit makes one) is `invalid_input` naming the fix, and the listing skips it. Learning never writes one.
  - **S4 dedups only a write that would be a candidate.** An active replacement that happens to equal a pending suggestion is written, not answered with the candidate's id: a user-directed save must never be deduplicated into something that is not in recall.
  - **`Partition::forget` keeps its signature;** `forget_as(targets, ForgetMode)` sits beside it, and `forget` is `forget_as(.., ForgetMode::Forget)`. S3 lives in the `Forget` arm. Every existing caller is unchanged.
  - **The flush report's `suggestions` changes meaning.** In L2 it counted decided-but-unwritten suggestions; now it counts suggestions **written** — candidates naming a note, from the user's words or inferred — as a subset of `candidates` (and of `stored`). `replaced` counts only automatic replacements that were written active.
  - **Deny is not tested here.** §8 L3 asks for `withdrawals_leave_sibling_notes_recallable` "once each for Keep mine, Deny, S6 and undo"; Deny's hook is L4's (§4.3), so L3 tests the service act it will call — undo of a suggestion, the same withdrawal as Keep mine — and S6 and undo of a replacement. L4 adds the Deny case.
  - **Where the GUI shows it.** The Suggestions card sits in the list column with Recently learned (`MemoryList`, composed by `MemoryView`), under the `Active` chip while nothing is searched, and both are narrowed to the scope on screen, as Recently learned already was. The detail's `Confirm` reads **Replace** for a suggestion and is disabled until the replaced note has been read (its revision is what it sends). `openalpaca memory suggestions` also takes `--format json`, like `list` and `search`.
  - **Commits.** The characterization is a commit of its own before §8.12's three, as L1 and L2 did (04 §1). The storage-and-service commit also switched the undo route to render the new result, because the service's return type changed under it; the route commit added the rest. The service's listing is its own module, `memory/service/suggestions.rs`.
  - **Fixture 25** (`25-suggestion-lifecycle.json`, row `q5`) runs in the file-level loader, which gained a `withdraw` op, `status` and `replace_policy` on a store, `target_revision` on a confirm, an extraction context's `origin`, and the `suggestion` and `effective_status` expectations (README updated). S6 and S7 are service behaviour and are the service tests', not the fixture's.
  - **Left for the lead** (outside this slice's writable documents): 03's §11 rows for L3 — §4 origins and §6 derived supersession (S1, S3, S9, S10), §6 recently learned and undo (LR12), §6's forget contract ("a withdrawal is not a forget"), §7a's list item, route table and `memory_update` refusals — are not folded into 03 yet, and neither is the undo response above. CLAUDE.md is untouched.
- **rev 3.4 (2026-09-24), L2's review fixed (from `388ed329`, one commit per finding: two tests that pin guards no test covered — LR9's same-stat hand edit read as `external`, rule 13's target clause — a broken doc link, and the change below).** One fix changes what rev 3.3 recorded:
  - **An inferred item naming a `user` note is counted as a suggestion, not attempted.** Rev 3.3's first bullet ended "for a `user` target the file layer still refuses it until S1, and it is counted `failed`": the store loop sent the candidate, the file layer answered `authority_conflict`, and the flush logged a WARN whose text said a *tool call* may not replace the note — the planned interim result reported as a failure, blaming the wrong actor. The store loop now takes that plan down the suggestion arm (`Plan::Inferred` with a `user` target: counted `suggestions`, one DEBUG line, nothing written), so `failed` means a real refusal again. L3 replaces the arm with the write once S1 lets a candidate name a `user` note; an inferred item against an `external` or `imported` note is still written as the candidate §2.5's table says. Test: `an_inferred_change_to_a_saved_note_is_a_suggestion_not_a_failure`.
- **rev 3.3 (2026-09-24), L2 implemented (`33bf9226..fa9f4f1b`: the characterization, index schema 2 and the lookup, the service plumbing, the extractor).** Deviations taken while building it, each recorded here rather than silently:
  - **A user-words suggestion is decided in L2 and written from L3 on** (§2.5's table, row three; **for the lead to confirm**). §8 L2 lands `ReplacePolicy` as plumbing only and S1/S2 in L3, so in L2 the file layer would refuse a candidate naming a `user` note (D5) and would *apply* an active supersede of an `external` or `imported` one (`extracted_user` ranks 4, above their 2 and 3), which LR9 forbids. The extractor therefore decides the suggestion — the change test, the target, the policy — and counts it (`suggestions` in the flush report, one DEBUG line per item) without writing it; L3 replaces that one arm with the write. Between the two slices such a change is not recorded at all; at L1 it was written as a keyless active note beside the saved one. An **inferred** item naming a target is written as the table says (a candidate with `supersedes`, which retires nothing); for a `user` target the file layer still refuses it until S1, and it is counted `failed`.
  - **`REQUEST_OVERHEAD_BYTES` goes from 4 KiB to 6 KiB.** The prompt fixture and two context rows at their cut measure 5.1 KiB (rev 3.2 measured 3.7 KiB against L1's shorter prompt). At the default 40,000 TPM the cap is `TPM × 1.6 − 6144` = 56.5 KiB and still never binds; the floor below which a keyed session stays due moves from about 2,200 to about 3,000 TPM (`an_idle_key_far_below_the_default_tpm_still_learns`, at 3,000, still passes).
  - **Task-output extraction's output budget goes from 512 to 1,024 tokens** (§2.7). Ten items with up to eight aliases and a key each do not fit 512 tokens, and a cut answer is unparsable JSON, so the whole run would be lost. The input cut and the cadence are unchanged, as §2.7 says.
  - **The characterization test is a commit of its own**, before §8.12's three, as 04 §1 asks and L1 did.
  - **Readings, not changes.** The lookup's queries come from the user's own rows, long ones included (their items are candidates, but the known notes still help rule 13 and `replaces`). The search runs with `historical` on and keeps only effectively active hits, so a note whose validity window ended still counts (§2.3 step 3, "whether or not they are recallable"). A listed note whose file changed behind the service's back is reported with origin `external`. An empty validity window (end equal to start) is dropped like an inverted one. In the store closure the rules run in the order 12, target resolution, 11 again, 13, the change test, 14. The key lookup is `MemoryService::learning_key_targets`; the index reads are `fact_keys`, `notes_citing` and `fact_key_winner`.
  - **Left for the lead** (outside this slice's writable documents): 03's §11 rows for L2 — the §7a Extraction row ("reads through `learning_view` only") and §3 rule 2 (LR17) — are not folded into 03 yet, and `docs/Daemon_Manual.md` says nothing yet about aliases, keys, automatic replacement of a learned note or where a `user.*` fact goes; §8.11 assigns L2 no manual.
- **rev 3.2 (2026-09-24), L1's review fixed (`21a8954f..f00bbc9a`, one commit per finding).** Two of the fixes go past what this document says, and are recorded here rather than taken silently:
  - **A chunk whose project directory no longer exists is skipped, and its turns are dropped** (review finding 2, `5934a4ab`; **for the lead or the owner to confirm**). §1.1 turns a row's `workspace_root` into the partition it learns into, and nothing checked that the directory was still there: learning a turn whose project had been deleted re-created the directory with a fresh `.openalpaca/` store, and learning now runs minutes to days after the turn (the boot catch-up). The chunk is now advanced past before any model call, with one INFO naming the session and the rows and a `skipped_root_gone` count — the same fate LQ4 gives a deleted conversation's turns. The alternative, learning such a turn into the home partition, is rejected: it breaks LR1 (a row with a root learns into that project) and would file project facts as global. The accepted residual: a project on a volume that is not mounted when learning reaches it is treated as deleted, and its turns are dropped too. The Daemon Manual says so beside the LQ4 sentence.
  - **§1.6's TPM cap covers the whole request** (review finding 7, `f00bbc9a`). The cap limited `in_bytes` (the rows and the known notes) to 40 % of TPM, but the headroom check of §1.7 estimates the whole request, and the fixed prompt plus two context rows add about 3.7 KiB; below about 4,300 TPM a full chunk with context never passed the half-bucket rule on an idle key, so §1.7's "a chunk can always start once the key is idle" was false there. The cap now subtracts `REQUEST_OVERHEAD_BYTES` (4 KiB, a constant beside §1.10's, pinned by a test that measures the prompt and the context at their cut): `in_bytes ≤ TPM × 1.6 − 4096`. At the default 40,000 TPM that is 58.5 KiB (§1.6 says about 62 KiB) and still never binds. A floor remains and is stated on `Budget`: below about 2,200 TPM the fixed prompt, its context and one cut turn alone pass half the bucket, and a session on such a key stays due.
  - **Not deviations, for the record.** The local model's call is registered for cancellation under the lock that reads the in-flight set, so a turn that starts between the quiet check and the call makes the flush yield before the call (finding 1, LR7 as written). A flush that made no call and moved no chunk logs "memory learning finished" at DEBUG, so a capped session's one INFO a day is the cap's own (finding 4, §1.8 as written). The owner check (`MemoryService::context`) and the usage row (`record_and_log`) run on the blocking pool (finding 5, §1.9 as written). `DELETE /v1/sessions/{id}`'s forget moved into `delete_session` and `DELETE /v1/chat/history`'s into `after_history_cleared`, so a test reaches each (finding 6).
- **rev 3.1 (2026-09-24), L1 implemented (`48f50793..f5cfc80b`).** Deviations taken while building it, each recorded here rather than silently:
  - **The L1 extractor's answer is `{"items": [...]}` with `rows`.** L1 keeps today's item fields and wording, but rule 1 (§2.4) and the citation rules 4–5 need the new top level and `rows`, so the prompt shows a numbered transcript (`R<n> user|assistant|queued-task (written by the assistant)|user (long; may quote other text)|context …`) and asks for both. L2's fixture replaces the wording; the shape is already L2's. `max_tokens` is the budget's `out_tok` (2,800 on a large window), not 512.
  - **One turn always fits, by a second cut.** §1.6 says the per-row cuts guarantee it, but on an 8,192-token window a cut user row (8 KiB) plus a cut answer (4 KiB) is over the 7,201-byte transcript. A turn alone over the budget is re-cut proportionally, head and tail around the same omission marker (`window::fit_group`).
  - **Yielding and the rate headroom apply to automatic flushes only.** `flush_session` (the harness, which is sequential) and the stop drain (after the server stopped) do not share the model with chat, so they neither yield nor wait for headroom; the 40 % TPM cap on the budget applies to every flush on a keyed model.
  - **A bad-output hold backs off too** (60 s doubling), like §1.4's other holds; attempts are logged at INFO and only the skip at WARN, which is the "one WARN" of `three_invalid_outputs_skip_the_chunk_with_one_warn`.
  - **An automatic flush that made no progress does not restart at once.** A cap, the rate headroom, a busy daemon or a yield leaves the session due for the next sweep; restarting at the end of the flush looped (found by a revert experiment, pinned by `a_flush_that_made_no_progress_waits_for_the_next_sweep`).
  - **Details.** The sweeper reads each watched session with `get_session` (N small) rather than one `IN (…)` query, which also finds a deleted session and forgets it (LQ4, covering the workspace purge); `DELETE /v1/sessions/{id}` and `DELETE /v1/chat/history` forget at once. `extract_min_content_len` counts the user's own rows only, so a chunk of assistant-written follow-ups alone is advanced past with no call. A turn landing in another session of its lane marks the session it left due (the gateway sees R48's project switch first).
- **rev 5 (2026-09-24):** §13 adds MD-14 (layered memory tools: index-first `memory_search`, batch `memory_read`, `memory_timeline`), MD-15 (index-first recall, `recall_mode`, measured against excerpts) and MD-16 (conversation summaries as `episode` notes, searchable, never auto-injected), adopted from claude-mem's `mem-search`/`how-it-works`; plan rows L10a–L10c and two L8 additions.
- **rev 4 (2026-09-24):** §12 adds MD-13 — `/remember`, `/forget`, `/memories` as deterministic, model-free chat commands on attended app/CLI turns, `/remember` minting origin `user` — and plan row L9.
- **rev 3 (2026-09-24):** the owner answered LQ1–LQ4; LQ1 (the reader judges, marked) rewrites §7.4 and the L7 guard tests from refusal to recording; the test model is `qwen3.8:27b-mlx`.
- **rev 1 (2026-09-24):** initial design and plan for Learning v2, written for MD-10–MD-12, the adopted scope and the recommended answers to LQ1–LQ4. It is grounded in three read-only surveys at `bdfcf272`/`55a1dc4c`, whose citations are restated here, and it adds rulings LR1–LR16.
- **rev 2 (2026-09-24):** applies a review of rev 1: 17 correctness findings (C1–C17) and 12 buildability findings (B1–B12). Each was checked against the code at `55a1dc4c`. All 29 are accepted, two of them in part (B1, B11); no finding is rejected whole. Rulings LR17–LR21 are added, and LR1, LR3, LR5, LR7, LR11–LR13 and LR15 are revised.
  - **C1 (major), accepted.** S6's dismissals were source-suppressing forgets that ran after the confirm, so a dismissal citing the confirmed note's row made that note `source_forgotten` (`partition.rs:940`, `:1156-1167`; `authority.rs:252`; `search.rs:223`) while its target was superseded. Dismissals are now withdrawals that suppress nothing, run after the write (S6); rule 11 also runs after target resolution. Test: `confirming_one_of_two_suggestions_sharing_a_row_keeps_it_recallable`.
  - **C2 (major), accepted.** Deny, Keep mine, S6 and undo were forgets, so they suppressed the whole message and silently retired sibling facts, which §1.4(d) then kept from being re-learned. `ForgetMode::Withdraw` now writes no links and no suppressed source. Undo widens to accept any candidate, and every "reject this change" act goes through it; explicit forget is unchanged (§3.3, LR12, LQ2). Sibling tests were added for each act.
  - **C3 (major), accepted.** A router error counted as an attempt, so an outage skipped windows, and quarantine and I/O errors were unhandled. Only a reachable model's bad output counts now; unavailability, quarantine (checked before the call) and store errors hold the window with backoff and one WARN per outage (§1.4, LR20). This is reconciled with P10 and fixture 19.
  - **C4 (major), accepted.** "A cloud call delays nothing" was false: the flush drains the key's shared TPM bucket (`retry.rs:36,64`, `rate_limiter/mod.rs:327-350`), and an `openai` provider on localhost never yielded. The design now has a quiet re-check before each chunk, local detection by a keyless provider or a loopback `base_url`, and a new read-only `LlmRouter::rate_headroom` that keeps half of each bucket for chat, with a budget cap at 40 % of TPM (§1.6, §1.7, LR7).
  - **C5 (major), accepted.** An `extracted_other` candidate could be confirmed in chat and labelled "from what you said". `confirm_suggestion`, the preview and `<memory_suggestions>` now require origin `extracted_user` and a user-authored cited row, and the date comes from that row. S10 stops a model-tool edit from laundering an inferred candidate (§4.2 rule 5, §4.4, §4.5).
  - **C6 (major), accepted.** One cited user row made an item `UserWords`. Now every cited non-context row must be user-authored, and the residual is stated (§2.4 rule 5, LR3; prompt rule 2).
  - **C7 (major), accepted.** The ambient `memory_read` grant reached plugin templates, whose tool results go to third-party processes. It now covers `AgentSource::Internal` only (§5, LQ3). Test: `a_plugin_template_gets_no_ambient_memory_read`.
  - **C8 (major), accepted, with the first fix offered.** LR1 filed project-less turns into the session's project, against 03 §7a and §3 rule 1. Migration 044 stores `workspace_root` per user row, and each row learns into its own root; a chunk never mixes roots (§1.1, §1.3, LR1). No 03 row changes for it.
  - **C9 (minor), accepted.** A re-run duplicated candidates, and a cancellation could separate the writes from the cursor advance. Rule 13 now covers pending candidates through the index's `note_source`, and the store loop and the CAS run in one blocking closure (§1.4, §1.7).
  - **C10 (minor), accepted.** Guard (b) needed an "Extraction surface" fact that nothing records, and it dropped a same-key correction. It is now defined by origin plus a cited chunk row (a `ModelTool` note counts), and it drops an item only when key or title **and** content match (§1.4(b)).
  - **C11 (minor), accepted.** `turn_started` now runs before the user row is persisted (§1.5). Test: `a_turn_is_in_flight_before_its_row_exists`.
  - **C12 (minor), accepted.** Piped or pasted text was the user's own words. Rows over 8 KiB now support only `OtherOutput` (LR21), the prompt tells the extractor that user rows may quote other text, and the residual is stated next to U2 (§1.2).
  - **C13 (minor), accepted.** `pending_suggestions` would have parsed partitions on every attended turn. Index schema 2 adds `note.supersedes` and `note_source` (LR18), `learning_view` runs on `spawn_blocking`, and both are in the service bench (§2.3, §4.5, §6.4).
  - **C14 (minor), accepted.** Archiving a suggestion activated its link. S9 refuses it on every surface.
  - **C15 (minor), accepted.** §8.10 gains the config tests (`config_schema/tests.rs:101`, `:156`; `daemon_config/tests.rs:586-608`; `daemon_config_cli/tests.rs:80`) and the confirmation-event literals, including the one pattern without `..` (`routes/chat.rs:604-615`). The registry count is stated: 43 → 48 → 47.
  - **C16 (minor), accepted.** A `user.*` fact learned in two projects piled up. The key lookup now spans the chunk's partitions, and `user.*` keys without a target are global (§2.5, LR17). The residual for other projects' keys is stated.
  - **C17 (minor), accepted.** The connector filter now cites `telegram/connector.rs:143-151`, `discord/connector.rs:234-240` and `imessage/connector.rs:198-204`.
  - **B1 (major), accepted in part.** A restated, unchanged fact became a suggestion or an automatic replacement. The deterministic change test is in §2.5. It differs from the proposed fix (suggest only on an explicit `replaces`) in one case: a saved note found by key that the model was **never shown** still yields a suggestion. There a real change ("switched to Helix") shares no term with the old note, and dropping it would lose exactly what MD-11 asks to surface; the suggestion shows both texts. The corpus gains `restate` turns and a restatement metric (§7.1, §7.3).
  - **B2 (major), accepted.** The key lookup covered one partition. It now covers the chunk's project partition, then home, and at least 2 `user.*` change pairs cross a project boundary in the corpus (§2.5, §7.1).
  - **B3 (major), accepted.** L1 is reordered into seven buildable commits, with the characterization tests alone in commit 3 and the key removal deferred to the extractor commit. The migration commit also regenerates `docs/api/` and CLAUDE.md, because `--check` is a CI gate (§8.12).
  - **B4 (major), accepted.** The learning phase is sampled at the reader's default temperature (`backend.rs:52,69,146`). The harness now runs the queue in `Manual` mode with defined `flush_session` semantics, subscribes to the bus around each flush, and adds a replay mode. Results are reported per pair, with the spread over three replays (§7.2, §7.3, §7.5).
  - **B5 (minor), accepted.** The lookup is now buildable: 512-character query windows (`query.rs:181-184`), step 2 through `note_source`, and rule 13 defined without a surface marker (§2.3).
  - **B6 (minor), accepted.** The prompt text is a committed fixture with a byte-for-byte test, the `<output_schema>` is given verbatim, `rows` accepts `3` and `"R3"`, context rows are dropped from citations, and rule 11 ties go to model order (§2.1, §2.2, §2.4).
  - **B7 (minor), accepted.** `MemoryIndex::now` stays private. Each hit carries `upcoming`, computed on the index's clock (§6.1).
  - **B8 (minor), accepted.** L4's file list names the seven full `ToolContext` literals, not the ~20 the review estimated (`PersonaToolContext` is another type), and its first commit converts them to `..Default::default()`.
  - **B9 (minor), accepted.** The same-stat test could not fail, because `ctime` moves. The cache takes an injectable stat function, and the test uses it (§6.3, L6).
  - **B10 (minor), accepted.** The subagent package edits changed nothing in production. They are dropped; one ring test pins the real path, and the empty-bundle path is residue (§6.2, LR15).
  - **B11 (minor), accepted in part.** Accepted: `restore_replaced` is deferred (§0.3); the switch trigger is derived in the sweeper from `session.status`, with no bus subscriber; five learning knobs become constants, leaving three `[orchestrator.learning]` keys plus `extract_max_calls_per_day`; E′ leaves L8. **Rejected: replacing the stop drain with the boot catch-up.** MD-10 is an owner decision that names "before the daemon stops", and removing it is the owner's call; the drain stays, as a constant (§10 *Noted*).
  - **B12 (minor), accepted.** The leakage check adds CJK 12-character shingles beside the word check, and the coverage judge gets a frozen, hashed prompt (§7.1, §7.3).
