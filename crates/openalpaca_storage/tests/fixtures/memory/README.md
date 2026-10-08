# Memory invariant fixtures

Data files for every row of the essential regression matrix in
`tasks/memory-redesign/04-implementation-plan.md` §5 (rev 4), written against the
contracts frozen in `tasks/memory-redesign/03-design.md` §2–§7a (rev 4). Slice 2
(`openalpaca_storage::memory_files` / `memory_index`) and slice 3 (the
`MemoryService`, the tools, the routes) load them; nothing here is executed on its
own, no model is involved, and no file in this directory is code.

The fixtures are the expectation, not the implementation: when a fixture and the
design disagree, the design wins and the fixture is a bug — fix the fixture in a
commit that names the design section. When a fixture pins something the design
leaves open, the file says so in `notes_for_implementer`, and the report that
landed these files lists each such point.

## Files

One JSON file per matrix row, numbered by the row's position in the table, plus
four query fixtures the plan names for slice 2 (`04-implementation-plan.md`,
*Slice 2*, "the query fixtures"), and a fifth that the index layout revision
added (design 09 §6.5.6, LQ17).

| File | Matrix row (plan §5) | Loads in |
|---|---|---|
| `01-cwd-never-selects-partition.json` | Global or connector request, daemon started by hand inside project A | slice 3 |
| `02-no-marker-dir-global-only.json` | CLI turn in a directory with no `.git` above it and never chosen as a project | slice 3 |
| `03-git-repo-lazy-partition.json` | CLI turn inside a git repository; the project's notes go to its folder under the home store | slice 2, 3 |
| `04-symlink-on-partition-path-refused.json` | A symlink, or a file, at `memory`, `owners/<key>` or `notes` in a project's folder under the home store | slice 2, 3 |
| `06-newer-database-refused.json` | Any slice-0-or-later build meets a newer database | slice 0 (landed), 3 |
| `07-hidden-parameters-refused.json` | The model supplies hidden owner, workspace or path parameters | slice 3 |
| `08-system-principal-no-partition.json` | A `/v1/command process` turn, its workflow and its subagents | slice 3 |
| `09-same-phrase-two-owners.json` | The same phrase in two owners or projects | slice 2, 3 |
| `10-authority-order-correction.json` | A user correction vs. a model-inferred related fact | slice 2, 3 |
| `11-validity-time-x-before-june-now-y.json` | "Used X before June, now Y" | slice 2, 3 |
| `12-query-handling-cjk-identifier-negation-punctuation.json` | A short Chinese term, a mixed code identifier, negation, punctuation | slice 2 |
| `13-read-after-write-and-freshness.json` | Save and then immediately ask; manual edit or delete; an external new file | slice 2, 3 |
| `14-concurrent-saves-conflict-large-note-duplicate-uuid.json` | Two agent saves; an edit conflict; a large note; a duplicate UUID | slice 2, 3 |
| `15-crash-at-each-recovery-step.json` | A crash at each step of design §6's recovery table | slice 2 |
| `16-forget-suppression-and-replacement.json` | Forget, erase the cache, re-run source extraction; forget a replacement | slice 2, 3 |
| `17-move-purge-factory-reset-config-reset.json` | Re-point while writing; a re-point interrupted and finished at boot; purge; factory reset; adopt by project id; config reset | slice 3 (3.7); storage design Stage 1 (4) |
| `18-malicious-content-stays-reference-data.json` | Malicious Markdown, frontmatter, source text, … cannot mint origin `user` | slice 2, 3 |
| `19-offline-no-llm.json` | No configured LLM, offline | slice 3 |
| `20-compaction-handoff-cancelled-task.json` | Compaction, lead/worker handoff, a cancelled task | slice 3 |
| `21-query-natural-question-prefer-vim.json` | slice-2 query fixture: "what editor do I prefer?" → "User prefers vim for editing" | slice 2 |
| `22-query-literal-and.json` | slice-2 query fixture: F4's literal-`AND` case | slice 2 |
| `23-query-cjk-lengths.json` | slice-2 query fixture: CJK queries of 1, 2 and 3+ characters; a Devanagari stem found inside its inflected form (LQ17's repair round 1) | slice 2 |
| `24-forgotten-replacement-not-revived.json` | slice-2 query fixture: a forgotten or archived replacement does not revive its predecessor | slice 2 |
| `25-suggestion-lifecycle.json` | Learning v2 L3 (design 09 §3): a suggestion retires nothing until a confirm bound to its target's revision; an automatic replacement's withdrawal brings the old note back; a suggestion is never archived or edited by a tool call | slice 2 (the file-level loader) |
| `26-query-prefix-in-body.json` | LQ17 query fixture (design 09 §6.5.6): "what do I prefer?" finds "User prefers kitty" through its body alone (`prefix`); a term inside a body word (`presso`, `now` in a Chinese note's heading) finds nothing; a Latin word glued to Chinese is a `term`; a literal's last token is a prefix only from three characters, or as a number | slice 2; step 2's recall also at slice 3 (`prompt_ctx/sources/memory.rs`) |

Row 06 is about the database opener, not the memory library; its file states the
expectation so the matrix is complete, and the loader that runs it is the database
test module, not the memory one. Row 05 (a schema-42 database refused) went with the
schema line's re-baseline at version 1 (`tasks/store-revision/design.md`): a
pre-squash database now meets row 06's refusal.

## How a loader runs a fixture

1. Create one temporary root. Every path in the fixture is relative to a named
   root inside it (below). Never `~/.openalpaca`, never the developer's store:
   `OPENALPACA_HOME_STORE` points at `<tmp>/home` for the whole run.
2. Set up the **database** rows the file names, the **workspaces**, the extra
   **directories**, the **partitions** with their notes and control snapshots, and
   the **daemon** facts (its CWD, its LLM configuration). A workspace a project
   partition names is registered (`store::projects::register`), as the daemon's
   first turn there would be; then the `home_links` and `home_files` are made.
3. Build each named **context** (`MemoryAccessContext`, design §7a) exactly as the
   file says. The loader resolves the project root from the request workspace
   path with the store's own resolver (`store::projects::resolve`, storage design
   §3): the nearest registered project or folder holding `.git`, never the home
   store; `.openalpaca` is no marker. The fixture never writes a resolved root.
4. Run the **operations** in order. Each carries its own `expect`; the run stops at
   the first failed expectation and reports the step.
5. Check **`final_state`**.

The library under test does the serializing, hashing and key derivation. The
fixture supplies inputs and the values those functions must produce, so a loader
that "helps" by computing a revision itself has silently stopped testing the
library.

## Top-level object

| Key | Required | Meaning |
|---|---|---|
| `row` | yes | `{ "id": <1..20 or "q1".."q5">, "scenario": <matrix text>, "required_outcome": <matrix text> }` |
| `design` | yes | the design sections the expectations are read from |
| `loads_in` | yes | `["slice-2"]`, `["slice-3"]` or both — which slice's test suite loads the file; `"slice-0"` marks the already-landed 0.5 refusal (file 06), whose test exists today |
| `notes_for_implementer` | no | prose: readings the file had to pin, and why |
| `owners` | yes | `[{ "id", "owner_key" }]` — every owner id the file mentions, with its derived key (below) |
| `database` | no | `{ "global_users": [ids], "local_user_id": id, "messages": [rowid strings], "tasks": [uuids] }` — the rows the identity check and source verification see; absent means the same as empty |
| `workspaces` | no | `[{ "name", "marker": ".git" \| null, "subdirs": [..], "symlinks": [{ "path", "target" }], "files": [{ "path", "content" }] }]` — `subdirs`, `symlinks` and `files` are relative to the workspace and are created in that order, after the marker — any other marker fails the load |
| `home_links` | no | `[{ "path": <proj: path>, "target": <@ path> }]` — symlinks inside a project's folder under the home store, made after every project partition's workspace is registered |
| `home_files` | no | `[{ "path": <proj: path>, "content" }]` — files there, made at the same point |
| `directories` | no | extra plain directories, by name (symlink targets, a moved-to location) |
| `daemon` | no | `{ "cwd": <fixture path>, "llm": "configured" \| "none", "network": "allowed" \| "denied" }` — `network: denied` means the harness runs under plan §6's no-network mechanism |
| `partitions` | no | see *Partitions* |
| `contexts` | no | name → `MemoryAccessContext`, see *Contexts* |
| `operations` | yes | see *Operations* |
| `final_state` | no | the same vocabulary as an operation's `expect`, checked after the last step |
| `foreign_ids` | no | UUIDs the file references on purpose that exist in no partition (a `not_found` probe, a tombstoned id that has no file). The cross-reference check accepts only these |

Any other top-level key (for example `unicode61_only_would_return` in fixture 21)
is informational for the implementer; a loader ignores it.

### Fixture paths

A fixture path is `<root>/<relative>`; the loader substitutes the root:

| Root | Meaning |
|---|---|
| `home` | the home store (`OPENALPACA_HOME_STORE`) |
| `ws:<name>` | the workspace directory `<name>` |
| `dir:<name>` | the extra directory `<name>` |
| `part:<name>` | the partition directory of partition `<name>` (`…/memory/owners/<owner-key>`) |
| `store:<fixture path>` | the project's folder under `home/projects/` named for that root — `store::projects::folder_name` of the canonical path (storage design R4–R7) — whether or not it exists |
| `proj:<name>` | the folder of workspace `<name>`'s project under the home store's `projects/` (the workspace is registered) |

A `daemon.cwd`, a context's `workspace`, a `repoint` target and every
`files_exist` / `files_absent` entry use this form.

### Owner keys, partition keys, revisions

- `owner_key` is the first 32 lowercase hex characters of SHA-256 over the UTF-8
  bytes of the owner id (design §2). The files carry the literal value so the
  loader's derivation is checked against it: `u_example_alice` →
  `32acea7eb739b08afd959db91e46d450`, `u_example_bob` →
  `5c357145f3a583c080c9f1d8ee520e71`, `system` →
  `bbc5e661e106c6dcd8dc6dd186454c2f`, `u_example_alice_reborn` →
  `9bfb77d3249642b6ba22fe7dcade07a0`.
- A home partition key is `home-<owner-key>` and is written literally. A project
  partition key is `proj-<owner-key>-<16 hex of sha256(the project id)>` (storage
  design R18); the id is minted when the project is registered, at run time, so the
  key is written as the placeholder `"@derived"`, as are a project snapshot's
  `project_id` and its `root` (recorded at the first write, never compared). A
  re-point does not change the key.
- A revision is the lowercase hex SHA-256 of the note file's exact bytes (design
  §4). A snapshot entry's `revision` is `"@current"` — the hash of the bytes the
  loader wrote for that note — unless the fixture gives a literal 64-hex value,
  which is then deliberately **not** the file's hash (an external edit already
  happened). The literal used for that is 64 × `f`.

### Placeholders

Any string beginning with `@` is a placeholder; a loader must resolve every one
it meets and fail on one it does not know.

| Placeholder | Where | Resolves to |
|---|---|---|
| `@current` | snapshot `notes[id].revision` | the hash of the note file as written |
| `@derived` | snapshot `partition_key`, `project_id`, `root` | the value the library derives for that partition |
| `@rev:<uuid>` / `@rev:<label>` | operation inputs | the note's current revision when the step runs |
| `@id:<label>` | anywhere after the step that defined the label | the UUID the service minted for the note stored with `"as": "<label>"` |
| `@from_rev`, `@to_rev` | `crash_at` expectations | the revision before the interrupted mutation, and the one it was writing |
| `@from_rev:<label>` | operation inputs | the revision the note had **before** the labelled `update` step ran (a deliberately stale revision) |
| `@ws:<name>`, `@dir:<name>`, `@part:<name>` | inputs that take a directory | the absolute path of that root |
| `@stale` | operation inputs | a syntactically valid 64-hex revision that is not the target's current one (the loader uses 64 × `f`) |

## Partitions

```json
{
  "name": "alice_home",
  "owner": "u_example_alice",
  "scope": "home" | "project",
  "workspace": "<workspace name, project scope only; a directory name when a re-point makes it the project's path>",
  "notes": [ <note>, ... ],
  "snapshot": <snapshot spec, optional>
}
```

The loader creates the partition the way the library does — `memory/`,
`owners/<owner-key>/`, `notes/`, `.history/` with `create_private_dir`, the
project `.gitignore` (`*\n`), `MEMORY.md` — then places the notes and writes the
control snapshot. A partition that is listed with no `notes` exists and is empty.

A partition with `"lazy": true` is **not created** by the loader: it only names
the partition the service is expected to create (or not) during the run, so
expectations can refer to it by name (`snapshot`, `snapshot_missing`,
`notes_on_disk`, `part:<name>`). It has no `notes` and no `snapshot`.

### A note

Every frontmatter field of design §4 by its own name, plus `body` (the Markdown
below the frontmatter, written byte-for-byte) and two **fixture-level** fields
that are not frontmatter and must never be written into the file:

| Field | Meaning |
|---|---|
| `origin` | the origin the control snapshot records for this note (`user`, `extracted_user`, `extracted_other`, `imported`, `external`) |
| `recorded` | default `true`. `false` = the file is dropped in by hand and the snapshot has **no** entry for it; the service must classify it `external` on its next scan |

Omitted optional frontmatter fields are absent (the canon writes `null` or an
empty list, design §4); `schema` is always 1 and is omitted from the fixtures.
`sources[].id` is a DB id as a string (`"4812"` for message rowid 4812, a UUID for
a task); the canon writes it as the scalar. `supersedes` is the file's list of
UUIDs.

The loader **serializes the note through the library's canonical emitter** and
records the resulting hash. A note may instead carry `"raw": "<the whole file>"`
and no other field but `id`, `origin`, `recorded` and `filename`: the loader
writes those bytes verbatim (a malformed file, a hand-edited one, a duplicate
id) under the given `filename` (required with `raw`, since the loader cannot
derive a slug from bytes it does not parse). `id` is still given beside `raw`
so the cross-reference check can see it; the bytes are what the library reads.

A `body` may be a generator object instead of a string, used only for size
limits: `{ "@repeat": { "text": "<unit>", "bytes": N } }` repeats `text` until
the body is exactly `N` bytes (cut on a char boundary).

### A snapshot spec

Absent: the loader writes the **default** snapshot — schema 1, the binding
(`partition_key`, `scope`, `owner_key`, `owner_id`, `root`, and for a project
partition its `project_id`), `created_at` / `updated_at` from the file's `clock`
or `2026-09-23T10:00:00Z`, `moved_from` null (parsed, never set — storage design
R19), `repaired_at` null, `pending` null, `notes` = every note with
`recorded != false` at `@current` with its `origin` and `written_at` =
`updated_at`, `tombstones` `[]`.

An object: the same default, then each present key replaces the default's:
`notes` entries are merged per id (an id mapped to `null` is removed, so the
file is unrecorded), and `tombstones`, `pending`, `moved_from`, `repaired_at`
replace whole. Tombstone and pending objects have exactly the §7a shape.

The string `"missing"`: no snapshot file is written — the partition is
quarantined (design §2). An object with only `"raw"`: those bytes are written as
the snapshot (an invalid one).

## Contexts

A named `MemoryAccessContext` (design §7a), in fixture terms:

| Field | Values |
|---|---|
| `owner_id` | a string, or `null` for "no principal" |
| `workspace` | a fixture path the request names as its workspace (the `x-workspace-path` header / `request_workspace_root`), or `null` = global only. The loader resolves the project root from it with `store::projects::resolve`; `daemon.cwd` is **never** consulted |
| `surface` | `user_surface`, `model_tool`, `extraction` |
| `origin` | `extraction` only: the origin the extractor records — `extracted_user` (the default, the user's own words) or `extracted_other` (assistant or tool output) |
| `capabilities` | `{ "read", "write", "forget" }` booleans |
| `unattended` | boolean; default `false`. Not a field of the context — the turn's declaration, which decides whether a `destructive_hint` tool prompts or is refused (design §7, *Capabilities*) |
| `attribution` | `{ "lane_key", "task_id", "request_id", "source" }`, any of them `null` |

A context whose `owner_id` is not in `database.global_users` resolves to **no
owner**: reads return nothing, writes and extraction are refused with
`no_owner`, and no partition directory is created for it (design §3).

## Operations

```json
{ "step": 3, "op": "search", "context": "alice_tool_project", "input": { ... }, "expect": { ... }, "as": "label" }
```

`step` is a label unique within the file — an integer, or `"1b"` for a step
inserted between two — and the list order is the run order. `context` is
required for every service-level op and ignored by the library-level and
environment ops. `as` labels the note a `store` created (or the `crash_at`
mutation, or an `update` whose pre-image a later step needs) for later `@id:` /
`@rev:` / `@from_rev:` references. `input` is the frozen wire shape of design
§7a where one exists.

**Slice 2 has no service.** For it, a service-level op is the library call
underneath: `search` is the index query with the context's partitions
pre-resolved and the §5 step-5 filters; `recall` is the same query with the
automatic-path filter set (same-key losers and `source_forgotten` notes out) and
the §4 byte budget, with no prompt assembly. The index serves no excerpt (design
09 §6.5.1), so an `excerpt_contains` a slice-2 search checks is read from the note
body at the hit's `chunk_ordinal`, as the service reads it from the verified file.
`store`/`update`/`forget`/`list`/`read` are `memory_files` mutations and reads on
the named partition with the context's `surface` mapped to the origin it
records. Ops slice 2 cannot run
(`extraction`, `tool_call`, the CLI and daemon ops) are skipped by name, and the
file's `loads_in` says which slice owns the whole file.

| `op` | `input` | Layer |
|---|---|---|
| `search` | the `memory_search` schema (`query`, `kind`, `scope`, `limit`, `historical`, `as_of`) | service; slice 2 runs it against the index with the context's partitions pre-resolved |
| `recall` | `{ "message": <the user turn>, "budget_bytes"?: N }` — the automatic per-turn path (design §5 steps 1–8) | service |
| `read` | the `memory_read` schema | service |
| `list` | the `memory_list` schema | service |
| `store` | the `memory_store` schema; on an `extraction` context also `status` (`active` \| `candidate`) and `replace_policy` (`direct`, the default, \| `suggest_unless_learned`, design 09 §3.2 S2) | service; slice 2 runs the same through `memory_files` with the partition named by `partition` |
| `update` | the `memory_update` schema, plus `target_revision` with `action: confirm` — the revision of the note a suggestion replaces (design 09 §3.2 S5) | service |
| `withdraw` | `{ "id", "expected_revision" }` — undo or a dismissed suggestion: the note goes, its tombstone keeps no link and suppresses no source (design 09 §3.3) | library |
| `forget` | the `memory_forget` schema, plus `"confirmation": "approved" \| "denied"` for the card an attended turn raises | service |
| `tool_call` | `{ "tool": <name>, "arguments": <raw JSON as the model sent it> }` — bypasses nothing: the full validation path | service |
| `extraction` | `{ "typed": <the message as typed>, "assistant_text"?, "tool_output"?, "attachment_text"?, "source": { "type", "id" }, "task_outcome"?: "completed" \| "failed" \| "cancelled" }` | service |
| `scan` | `{}` — one reconciliation scan (`POST /v1/memories/sync`) | library |
| `external_edit` | `{ "partition", "id", "append"?: text, "replace_body"?: text, "frontmatter"?: { field: value } }` — edits the file on disk behind the service's back | library |
| `external_delete` | `{ "partition", "id" }` | library |
| `external_create` | `{ "partition", "note": <note object, `recorded` forced false> }` | library |
| `erase_cache` | `{}` — delete `state/cache/memory-index-v1.sqlite` | library |
| `restart` | `{}` — drop the service/index handles and open again; recovery runs (design §6) | library |
| `crash_at` | `{ "mutation": "create" \| "update" \| "archive" \| "confirm" \| "forget", "after_step": 1 \| "2a" \| 2 \| 3, "input": <that mutation's input>, "then_external_edit"?: text }` — run the mutation and stop after the named step of design §6's table. `"2a"` is inside step 2: the `.history` copy is written and fsynced, the note's rename has not happened | library |
| `concurrent` | `{ "ops": [ <op>, <op> ] }` — run both at once; each has its own `expect` | service |
| `open_database` | `{ "stamped_version": N, "path": <fixture path> }` — create a fresh database there, stamp `schema_version` to N, reopen. The database opener, not the memory library | database |
| `recently_used` | `{ "limit" }` — `GET /v1/memories/recently-used` | service |
| `remove_partition` | `{ "partition" }` — the harness deletes that partition's directory and snapshot outright (an owner clearing a collision by hand before `adopt`) | harness |
| `ensure_project_store` | `{ "workspace": <fixture path> }` — the artifact path's first write for that project, which creates `artifacts/` in the project's folder under `projects/` and nothing else | store |
| `begin_pending_write` / `end_pending_write` | `{ "partition" }` — hold a memory write of that partition in flight (the quiesce hook's view of "pending") | service |
| `repoint` | `{ "from": <fixture path>, "to": <fixture path>, "crash_after_step"?: 1..3 }` — `PATCH /v1/workspaces`: storage design §6's four steps (1 `project.json` marked with `previous_root`, 2 the rows rewritten, 3 `projects/<name>` renamed, 4 the marker cleared). A crash after step 1, 2 or 3 leaves `project.json` carrying `previous_root`; the next `boot` finishes it (R24) | service |
| `purge` | `{ "workspace" }` — `POST /v1/workspaces/purge` | service |
| `factory_reset` | `{}` — `openalpaca config reset --factory` with the daemon stopped | CLI |
| `config_reset` | `{}` — plain `openalpaca config reset` | CLI |
| `boot` | `{ "local_user_id" }` — the next daemon start, minting or reading the owner; it first finishes every re-point a `project.json` still records as in flight (storage design R24) | service |
| `doctor` | `{}` — the read-only report | library |
| `doctor_repair` | `{}` — local, daemon stopped | library |
| `adopt` | `{ "owner_key" }` — local, daemon stopped | library |

## Expectations

Every key is optional; the loader checks the ones present. Ordered lists are
ordered; `*_unordered` lists are sets.

| Key | Checks |
|---|---|
| `ids` | the returned note ids, in this order (only where the design's rule determines the order: an exact title/alias hit before a term hit, a higher authority before a lower one, a same-key winner and no loser) |
| `ids_unordered` | the returned set, exactly |
| `ids_absent` | none of these ids is returned |
| `ids_include` | every one of these ids is returned, others allowed |
| `ids_may_include` | these ids may or may not be returned (a new external file before the next scan); nothing else is allowed beyond them and `ids_include` |
| `ids_absent_by_status` | no returned note has one of these statuses |
| `count` | `count` in a `memory_search` result, or the list length |
| `partial` | the `partial` flag |
| `index_state` | `ready` or `rebuilding` |
| `results` | `{ "<id>": { "match_includes": [MatchReason], "conflict": bool, "freshness", "status", "origin", "scope", "excerpt_contains", "superseded_by", "valid_from", "valid_to", "sources_status" } }` — per-result fields |
| `refused` | the operation is refused with this error class (below); nothing is written |
| `message_contains` / `message_not_contains` | substrings of the refusal or diagnostic message |
| `message_contains_once` | substrings that occur in the message exactly once |
| `created` | a `store` succeeded; with `scope`, `origin`, `superseded` (list of ids) from its result |
| `revision_changed` | whether the target's revision differs after the step |
| `current_revision` | a refusal's reported current revision equals this note's |
| `status` | for `update`, the result's `status`; for `forget`, the per-target `results[].status` map `{ "<id>": "forgotten" \| "revision_conflict" \| "not_found" }` |
| `note_status` | `{ "<id>": NoteStatus }` — the note's status read back through the library after the step |
| `not_served_from_cache` | the result text came from the re-read, re-hashed file, not from indexed text (§5 step 7) |
| `truncated`, `body_bytes_lte` | a `memory_read` result's `truncated` flag and body size |
| `frontmatter_aliases` | a `memory_read` result's `frontmatter.aliases`, exactly |
| `trust_label_present`, `provenance_fields` | the delivery carries the retrieved-data trust label; each delivered item carries every named field (§5 step 7) |
| `memory_used_includes` | these `{ id, reason }` pairs appear in `memory_used`, others allowed |
| `items_include` | a `recently_used` listing contains these items (partial match per item) |
| `preview_contains` | substrings of the factory-reset preview text |
| `adopted_partitions_count` | the number of partitions `adopt` reports renaming |
| `retained` | the `forget` result's `retained` object: `{ "transcript": bool, "session_logs": bool, "note_contains": text }` |
| `snapshot` | `{ "<partition>": { "notes": { "<id>": { "revision": "@current" \| "@from_rev" \| "@to_rev" \| hex, "origin" } \| null }, "tombstones": [..], "tombstones_count": N, "pending": null \| {..}, "moved_from", "owner_key", "owner_id", "root_is": <fixture path> } }` — a partial match of the control snapshot on disk (a project snapshot's `root` is recorded at its first write and never compared, so `root_is` is for a home snapshot); a note mapped to `null` must be absent from `notes` |
| `snapshot_missing` | `["<partition name>"]` — no snapshot file exists for that partition's (derived) key |
| `snapshot_unchanged` | `["<partition name>"]` — that partition's snapshot file has the same bytes after the step as before it (a re-point touches no snapshot) |
| `same_partition` | `[["<partition name>", "<partition name>", …]]` — each group derives one partition key (the project's id, storage design R18), so one snapshot file and one set of notes serve every name in it |
| `project_file` | `{ "<store: fixture path>": { "root_is": <fixture path>, "previous_root": <fixture path> \| null } }` — a partial match of that folder's `project.json`: `root` is the given path; `previous_root` is absent (`null`) or the given path |
| `files_exist` / `files_absent` | fixture paths |
| `file_content` | `{ "<fixture path>": "<exact bytes>" }` |
| `file_unchanged` | `["<fixture path>"]` — SHA-256 before and after the step are equal |
| `notes_on_disk` / `notes_not_on_disk` | `[{ "partition", "id" }]` — the note's file under `notes/` (whatever its slug) |
| `history_has` / `history_absent` | `[{ "partition", "id", "revision": "@to_rev" \| "@from_rev" \| "@current" }]` — `.history/<id>/<revision>.md` |
| `history_count` | `{ "<partition>": { "<id>": N } }` |
| `filename_matches` | `{ "<label or id>": "<regex>" }` — the basename of the note's file |
| `path_within` | `{ "<label or id>": "part:<name>/notes" }` |
| `warnings` | `{ "count": N, "contains": [text] }` — WARN lines this step logs |
| `events` | `[{ "type": "memory_changed", "change", "note_id", "origin", "surface" }]` — published this step, as a set; `events_count` for the number |
| `memory_used` | `[{ "id", "reason" }]` on the turn's `done` |
| `total_bytes_lte`, `chunks_lte`, `pinned_bytes_lte`, `truncated` | recall budgeting (design §4 limits) |
| `delivered_as` | `quoted_data` — the recall section is data in user/tool context, never a system instruction |
| `network_calls`, `model_calls` | integers, `0` for every memory operation |
| `doctor` | `{ "quarantined": [partitions], "orphans": [owner keys], "conflicts": [{ "type": "supersedes_authority" \| "same_fact_key", "note", "target" }], "malformed": [{ "partition", "file_contains" }], "duplicate_ids": [[ids]], "idless": [partition keys of project snapshots that carry no project id] }` — partial match of the report; a list given is matched exactly as a set |
| `owner_id_after` | `identity.local_user_id` after a reset |
| `extraction` | `{ "count": N, "count_lte": N, "notes": [{ "origin", "status", "body_contains" }], "unavailable": bool }` |
| `dedup` | `"created"` (a new note) or `"existing"` (exact dedup returned the existing id) |
| `suggestion` | a `store` wrote (or deduplicated to) a suggestion: a candidate naming the note it would replace (design 09 §3.1) |
| `effective_status` | `{ "<id>": EffectiveStatus }` — the note's derived status (`active`, `superseded`, `archived`, `candidate`) read back through the library after the step |

### Error classes

The §7a refusal names are used as written; classes the design names in prose
but gives no wire code are prefixed `fixture:` so slice 3 binds them explicitly
rather than guessing:

| Class | Design |
|---|---|
| `invalid_input` | §7a `memory_store` / `MEMORY_INVALID_INPUT`; §3 rule 6 (unknown fields refused) |
| `not_found` | §7a / `MEMORY_NOT_FOUND` |
| `revision_conflict` | §7a / `MEMORY_REVISION_CONFLICT` |
| `authority_conflict` | §4, §7a: a `ModelTool` supersede/edit/archive of a `user` note at the file layer (`ReplacePolicy::Direct`, the fixtures' `alice_tool`); on the service's tool path a supersede, or an edit of its content, is a suggestion instead (owner decision LQ11, 03 rev 7.4) |
| `confirm_refused` | §4, §7a: `confirm` from a `ModelTool` context |
| `not_undoable` | §6 / `MEMORY_NOT_UNDOABLE` |
| `quarantined` | §2 / `MEMORY_QUARANTINED` |
| `fixture:no_owner` | §3: the owner is not a `global_user` row |
| `fixture:capability_denied` | §7a `capabilities`: the context lacks the capability the operation needs (a worker's write) |
| `fixture:path_refused` | §2 / plan 3.2: a builtin file tool refuses `state/` or a partition's control files |
| `fixture:confirmation_unavailable` | §7 *Capabilities*: a `destructive_hint` tool on an `unattended` turn (the sandbox's M6 refusal) |
| `fixture:newer_schema` | plan slice 0.5; `tasks/store-revision/design.md` §1.4 (the one refusal, above this build) |
| `fixture:write_pending` | storage design §6 *Re-point*: refused while a memory write of the project is pending (the route answers `WORKSPACE_BUSY`) |
| `fixture:duplicate_id` | §6 *Management operations*: `doctor --repair` refuses on a duplicate id |
| `fixture:adopt_collision` | §6 *Management operations*: `adopt` refuses on a collision |

## Readings the fixtures pin

Each is a point where design rev 4 is silent or can be read two ways. The
fixture takes one reading and says so in its `notes_for_implementer`; the owner
signs these with the frozen set or sends them back as design revisions.

| # | Reading | Fixture |
|---|---|---|
| P1 | An explicit `scope: project` from a request that resolves no project root is refused `invalid_input`, not written to global | 02 |
| P2 | `as_of` alone never includes superseded or archived notes; `historical: true` is the only switch; a bound is half-open at the instant | 11 |
| P3 | A search result's `conflict: true` marks the note that **carries** an un-honored `supersedes` link, and the same-`fact_key` **loser**; the winner and the link's target read `conflict: false` | 10, 18, 24 |
| P4 | `memory_search` returns a same-`fact_key` loser flagged `conflict: true`; automatic recall excludes it (§3 rule 3 and §5 step 5 read together) | 18, 24 |
| P5 | The substring pass for 1–2 character queries has no `MatchReason`; those steps check ids only | 12, 23 |
| P6 | `source_forgotten` has no §7a wire field; the fixtures read it from a search result's `sources_status` | 16 |
| P7 | A `ModelTool` write from a context whose `capabilities.write` is false is refused `fixture:capability_denied`, naming `memory_write` | 20 |
| P8 | Exact dedup (same owner, same partition, same bytes) answers with the existing note's id rather than creating a second file; the `dedup` expectation reads it | 09 |
| P9 | The `authority_conflict` and `confirm_refused` messages name the Memory view and the `openalpaca memory` verb | 10, 18 |
| P10 | An extraction on a daemon with no configured model reports `unavailable` with one WARN and the turn is unaffected | 19 |
| P11 | `doctor` lists a project snapshot that carries no project id — an earlier build's (storage design R22) — by its key and the directory it was bound to; nothing opens, adopts or deletes it | 17 |
| P12 | A `forget` whose target id no longer exists answers `not_found`, not `revision_conflict`, whatever revision was sent | 16 |
| P13 | A `memory_changed` published by a reconciliation scan (`change: external`) carries no `surface`: §7a's enum is `user \| model_tool \| extraction` and a scan is none of them; the fixture omits the field until the design adds a value or makes it optional | 13 |

## Invariants every file satisfies (checked before commit)

- Parses with `python3 -m json.tool`.
- Every UUID that any operation, expectation, `supersedes` list, tombstone or
  snapshot names exists in one of the file's partitions, or is defined by an
  earlier step's `as` label, or is listed in `foreign_ids`.
- Every `context` an operation names is defined in `contexts`; every `partition`
  and `workspace` named anywhere is defined.
- Every owner named by a partition or a context appears in `owners`, and its
  `owner_key` is the derivation above.
- Owner ids, UUIDs, paths and message ids are examples: no real name, no real
  path outside the temporary root, no secret.
