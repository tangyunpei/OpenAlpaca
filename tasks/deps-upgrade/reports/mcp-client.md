# MCP client: rmcp 0.16.0 → 3.4.1

Group: **MCP client**. Worktree `SP/wt-rmcp` (detached at `5e5ac317`). Target dir `SP/target-198`.
`SP` = `/private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/4b1ee495-f087-45a9-9fa8-4325993552c8/scratchpad`.

Evidence tags: **[M]** measured (command + log named), **[C]** read in our code (file:line),
**[D]** read in dependency source (`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/<crate>-<ver>/…`),
**[Docs]** inferred from docs/changelog/web.

## 1. Bottom line

* The bump is **done and compiles**: `SP/patches/rmcp.diff` (720 lines; code +112/−178 = 290 lines across 14 files; `Cargo.lock` +52/−10).
  `openalpaca_mcp`, `openalpaca_core` and `openalpacad` all `cargo +1.98.1 check --all-targets` clean (0 errors; the daemon's single warning is the
  unrelated `proc-macro-error2` future-incompat from `teloxide → aquamarine`, present at every stage) **[M]** (`logs/rmcp-04`, `-06`, `-07`).
* Tests green on 3.4.1 **[M]**: `openalpaca_mcp` 34 passed / 2 ignored (npx e2e) (`logs/rmcp-08`); `openalpacad managers::mcp` 51 passed (`logs/rmcp-13`);
  `openalpaca_core --lib` 2023 passed / 5 ignored (`logs/rmcp-14`); `openalpacad routes::tools` 8 passed (`logs/rmcp-15`).
* rmcp 3.4.1 declares MSRV 1.88 **[D]** (crates.io index `rust_version`), and the bumped crate also checks clean on **1.93.0** **[M]** (`logs/rmcp-18`),
  so this bump does **not** depend on the toolchain bump and can land before or after it.
* The migration is almost entirely mechanical: struct literals of types that became `#[non_exhaustive]`, `Content/RawContent → ContentBlock`,
  and `ServerPeerInfo.server_info` becoming `Option`. No trait we implement changed shape (`ClientHandler`, `Transport`), no feature name changed.
* Behaviour changes the daemon will feel are few and mostly improvements (§4). **None touches persisted data, secrets storage or the keychain.**
  The wire to MCP servers changes (protocol version offered 2025-03-26 → 2025-11-25, a new `MCP-Protocol-Version` HTTP header).
* Two pre-existing defects surfaced while measuring, **not fixed and not caused by the bump** (§6): a cross-origin redirect replays
  `X-API-Key`/`extra_headers` secrets, and the reconnect ladder can loop without bound on a deterministic retriable error.

## 2. What I ran

| # | Command (all prefixed `nice -n 10 env CARGO_TERM_COLOR=never CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=SP/target-198`) | Result | Log |
|---|---|---|---|
| 0 | baseline `cargo +1.98.1 check -p openalpaca_mcp --all-targets --features test-utils` | 0 err, 0 warn | `rmcp-00` |
| 1 | bump requirement `"0.16"`→`"3.4.1"`, `cargo +1.98.1 update -p rmcp` | rmcp/rmcp-macros 3.4.1, process-wrap 9.1.0→10.0.1, +base64 0.23.1, +darling 0.24.1 (+core, +macro) | `rmcp-02` |
| 2 | check openalpaca_mcp (no fixes) | **12 error diagnostics** (E0639×6, E0308×2, E0609×2, E0560×1, E0432×1; 10 distinct sites) | `rmcp-03` |
| 3 | check openalpaca_mcp after fixes | 0/0 | `rmcp-04` |
| 4 | check openalpaca_core --all-targets (no fixes) | **20 error diagnostics** (E0639×17, E0432×2, E0609×1) | `rmcp-05` |
| 5 | check openalpaca_core after fixes | 0/0 | `rmcp-06` |
| 6 | check openalpacad --all-targets (2 same-pattern sites fixed by grep beforehand) | 0 err, 1 unrelated warn | `rmcp-07` |
| 7 | `cargo +1.98.1 test -p openalpaca_mcp --features test-utils` | 34 passed, 2 ignored | `rmcp-08` |
| 8 | `cargo +1.98.1 test -p openalpacad managers::mcp` | 51 passed | `rmcp-13` |
| 9 | `cargo +1.98.1 test -p openalpaca_core --lib` | 2023 passed, 5 ignored | `rmcp-14` |
| 10 | `cargo +1.98.1 test -p openalpacad routes::tools` | 8 passed | `rmcp-15` |
| 11 | `cargo +1.98.1 clippy -p openalpaca_mcp --all-targets --features test-utils`, before and after | 1 warning both times (`result_large_err` on `serve_with_conn`: 504 B before, 376 B after) — not introduced | `rmcp-16`, `-17` |
| 12 | `cargo +1.93.0 check -p openalpaca_mcp --all-targets --features test-utils` | 0/0 | `rmcp-18` |
| 13 | intermediate versions × {unchanged code, final code} | table in §3 | `rmcp-20-*` |
| 14 | four staged patches rebuilt from scratch, each: check mcp/core/daemon `--all-targets` + mcp tests | all clean, 34 passed each; stage 4 is byte-identical to `rmcp.diff` | `rmcp-30-*` |
| 15 | stdio probe: a Python stub MCP server logging every inbound line, driven by `McpClient`, on 0.16 and 3.4.1 | §4 | `rmcp-09`, `-10`, `rmcp-research/probe/log-*.txt` |
| 16 | streamable-HTTP probe (Python stub, JSON response mode; modes ok/401/401-bare/403) on 0.16 and 3.4.1 | §4 | `rmcp-11`, `-12`, `probe/http-*.txt` |
| 17 | cross-origin 307 probe on 0.16 and 3.4.1 | §6 | `rmcp-50`, `-51`, `probe/redirect-*.txt` |
| 18 | reconnect-ladder probe (`max_reconnect_attempts = 3`, tool returns `{"content": []}`), 0.16 and 3.4.1 | §4/§6 | `rmcp-52-*`, `probe/loop-*.txt` |
| 19 | `cargo tree` closure and `-d` duplicates, before/after, and with `default-features = false` | §5 | `rmcp-40-*`, `rmcp-41-*` |

Probe harnesses (`zz_*.rs`) were copied into `crates/openalpaca_mcp/tests/` only while running and moved out again
(`SP/rmcp-research/stash/`); they are **not** in the patch. The staged runs used `git apply -R` / `git restore` on the worktree
only; the final worktree diff equals `SP/patches/rmcp.diff` (checked with `cmp`).

**Orphans.** `managers::mcp` hang tests leave `sleep 3600` children. After my run (14:44:21–14:46:26) two new ones with ppid 1 had
start times inside that window — **I killed pids 42557 (14:46:07) and 43127 (14:46:15)**. Caveat, stated honestly: another session
produced identical pairs at 14:31–14:35 and again at 14:47:12/14:47:22 while none of my runs executed those tests, so the attribution
of the two I killed rests on the time window alone. I killed nothing else; the others (41 at last count) are not mine.

## 3. API map 0.16 → 3.4.1 and where each break arrived

Measured by compiling the **unchanged** code and the **final** code against each version (`logs/rmcp-20-*-summary.txt`) **[M]**, API
facts read in each version's source **[D]**.

| What we use (0.16) | 3.4.1 equivalent | Broke at | Our sites |
|---|---|---|---|
| `Implementation { name, version, ..Default::default() }` | `Implementation::new(name, version)` (struct `#[non_exhaustive]`) | 1.0.0 | `client.rs` default + 2 tests, `tests/filesystem_e2e.rs`, `openalpaca_core/tests/mcp_integration.rs`, `openalpacad/src/managers/mcp.rs:2315` |
| `CallToolRequestParams { meta, name, arguments, task: None }` | `CallToolRequestParams::new(name).with_arguments(map)`; `task` removed (SEP-2663 tasks became an extension), new `input_responses`/`request_state` (MRTR) | 1.0.0 (literal), 3.0.0 (`task`) | `client.rs` `call_tool` |
| `InitializeResult { … }` literal (test stub) | `InitializeResult::new(caps).with_protocol_version(..).with_server_info(..)` | 1.0.0 | `client.rs` test stub |
| `ToolAnnotations { hint: Some(b), ..Default::default() }` | builder `ToolAnnotations::new().read_only(b)…` or `from_raw(title, read_only, destructive, idempotent, open_world)`; serde shape **identical** (camelCase, skip-if-none) | 1.0.0 | 18 sites in `openalpaca_core` (4 production in `tools/builtins/mod.rs`, 1 in `tools/config/annotations.rs`, rest tests) + 1 daemon test |
| `Content = Annotated<RawContent>`, match on `content.raw`, `Content::text` | `ContentBlock` enum (`Text/Image/Audio/Resource/ResourceLink`, `#[non_exhaustive]`), `ContentBlock::text`, `.as_text()` | 2.0.0 | `openalpaca_mcp/src/lib.rs` re-export, `openalpaca_core/src/tools/mcp/bridge.rs`, e2e test |
| `running.peer_info().server_info: Implementation` | `ServerPeerInfo.server_info: Option<Implementation>` (a `server/discover` answer need not carry it; legacy `initialize` always fills it) | 3.0.0 | `client.rs` `do_handshake` |
| `handler.serve(transport)` (`ServiceExt::serve`) | unchanged; still the legacy `initialize` handshake (`ClientLifecycleMode::Initialize`) **[D]** `rmcp-3.4.1/src/service/client.rs:638-721` | — | — |
| `ClientHandler::on_{tool,resource,prompt}_list_changed` | unchanged signatures | — | `NotifyingHandler` |
| `Transport<RoleClient>` / `IntoTransport` (test `WatchedTransport`) | unchanged | — | client tests |
| `RunningService::{close_with_timeout, peer, peer_info, list_all_tools, call_tool}` | unchanged names; `call_tool` now drives MRTR `input_required` rounds (≤10) | — | client |
| `ServiceError` variants | + `SubscriptionLagged`, + `InputRequiredRoundsExceeded`; our `From` has a catch-all → `Sdk` | 3.0.0 | `error.rs` (no code change) |
| `StreamableHttpClientTransport::with_client(reqwest::Client, StreamableHttpClientTransportConfig::with_uri(..))` | unchanged | — | `transport/http.rs` |
| `StreamableHttpError::{AuthRequired, InsufficientScope}` (401/403 **with** `WWW-Authenticate`) | unchanged variants and mapping **[D]** `rmcp-3.4.1/src/transport/common/reqwest/streamable_http_client.rs:97,112,212,227`; measured end to end §4 | — | `error.rs::unauthorized_status` |
| `TokioChildProcess::new(cmd)` | unchanged (only the long-deprecated `split()` removed; stderr still inherited, kill-on-drop unchanged) **[D]** diff of `transport/child_process.rs` | — | `transport/stdio.rs` |
| features `client`, `transport-child-process`, `transport-streamable-http-client-reqwest` | same names **[D]** index features | — | `Cargo.toml` |

**Intermediate stops — yes, three are worth it** **[M]** (`logs/rmcp-30-summary.txt`): each stage below was rebuilt from a clean tree,
`check --all-targets` clean on mcp/core/daemon, and 34 mcp tests pass.

| Stage | rmcp | Code change | Protocol offered in `initialize` |
|---|---|---|---|
| 1 | 0.17.0 | **none** (manifest + lock only) | 2025-06-18 **[D]** `LATEST` |
| 2 | 1.8.0 | non-exhaustive constructors (+74/−153, mostly `ToolAnnotations` builders) | 2025-11-25 **[D]** (1.5.0 "add 2025-11-25 protocol version support" **[Docs]**) |
| 3 | 2.2.0 | `ContentBlock` (+18/−18) | 2025-11-25 |
| 4 | 3.4.1 | `server_info: Option` + doc citations (+19/−6) | 2025-11-25 **[M]** |

The 3.x-ready code does **not** compile against 2.x or earlier (`logs/rmcp-20-fixed-summary.txt`), so the stages must be applied in order.

## 4. Protocol and behaviour the daemon will feel

| Topic | 0.16 | 3.4.1 | Evidence |
|---|---|---|---|
| Handshake | `initialize` → `notifications/initialized` | identical sequence; **no `server/discover` probe** (only `serve_with_lifecycle(Discover/Auto)` would send one) | **[M]** probe logs `log-016.txt`, `log-341.txt`; **[D]** `service/client.rs:642-705` |
| `protocolVersion` offered | `2025-03-26` | `2025-11-25` | **[M]** |
| Version the server answers | accepted as-is, no validation (even `2099-01-01`) | same | **[M]** (four reply modes) |
| `clientInfo` sent | `{"name":"rmcp","version":"0.16.0"}` | `{"name":"rmcp","version":"3.4.1"}` — our configured `client_info` ("openalpaca-mcp") is **never sent** in either (pre-existing: `NotifyingHandler` does not override `get_info`) | **[M]**; **[C]** `client.rs:68-92` |
| `capabilities` sent | `{}` | `{}` | **[M]** |
| `ping` | no params | `params._meta.progressToken` | **[M]** |
| `tools/list` pagination | cursor loop | same loop, both pages fetched | **[M]** |
| Tool `inputSchema` → LLM `parameters` | passed through | byte-identical JSON (keeps `$schema`); schema stripping in 3.x is server-side only | **[M]** probe output; **[D]** `handler/server/common.rs:75` |
| `outputSchema`, annotations | parsed | parsed identically | **[M]** |
| `{"content": []}` tool result | **deserialization error → retriable `ServerInternal` → reconnect**; with the production default of 3 attempts the ladder **looped: 32 respawn+initialize+call cycles in 4 s** and was still going | `Ok("")` after one call | **[M]** `rmcp-52-loop-*`, `probe/loop-*.txt` |
| structuredContent-only result | accepted; our bridge returns `""` (drops it) | same | **[M]**; **[C]** `bridge.rs:84-104` (pre-existing gap) |
| Tool-list-changed notification → refresh | works | works (daemon test `a_tool_the_server_adds_is_registered_under_the_same_generation` with a real notifying stdio stub passed); rmcp now also invalidates its response cache **before** our handler runs | **[M]** `rmcp-13`; **[D]** `service.rs:1632`, `service/client.rs:348-364` |
| Client response cache (SEP-2549) | none | on by default, but writes only when the server sends `ttlMs > 0` (default TTL 0) — legacy servers are never cached; with `serve_stale_on_error = true` a modern server's cached `tools/list` could be served instead of an error | **[D]** `service/client/cache.rs:53-63,313-330`; disable with `peer.set_response_cache_config(ClientCacheConfig::disabled())` |
| Timeouts | our `with_cancel_and_timeout` wraps each op; rmcp adds none | same; a dropped request sends no `notifications/cancelled` in either | **[C]** `client.rs:593-616`; **[D]** `service.rs:561-591` |
| Stdio child | spawned via process-wrap 9, stderr inherited, killed on drop | process-wrap 10 (internal API change only), same semantics | **[D]** |
| HTTP: `MCP-Protocol-Version` header | not sent | sent on every post-initialize request, value = the version the server negotiated | **[M]** `http-*.txt`; **[D]** `streamable_http_client.rs:113-130` |
| HTTP: `Mcp-Session-Id`, GET SSE stream, DELETE on close | yes | yes (same sequence) | **[M]** |
| HTTP 401/403 with `WWW-Authenticate` at bring-up | `Unauthorized(401/403)` | same → `NeedsAuthorization` classification unchanged | **[M]** |
| HTTP 401 **without** header | `HandshakeFailed(… UnexpectedContentType(None))` | `HandshakeFailed(… UnexpectedServerResponse("HTTP 401 Unauthorized: "))` — same classification (`Unreachable`), different text in the failure detail | **[M]** |
| HTTP expired session (404) | error → our reconnect ladder | rmcp re-initializes **transparently** and retries the rejected POST once (`reinit_on_expired_session: true` default) | **[D]** `streamable_http_client.rs:2064-2075,2162-2176` |
| HTTP concurrency | serial | up to 16 concurrent POSTs — invisible to us, `McpClient` holds its service mutex across each call | **[D]**; **[C]** `client.rs:553-560` |
| HTTP SSE event size | unbounded | 16 MiB per event cap | **[D]** `client_side_sse.rs:18` |
| Legacy SSE transport | not used | not used (config only accepts stdio/http) | **[C]** `config/mcp.toml` |
| Server→client requests (sampling, roots, elicitation) | method-not-found / empty roots / decline | same defaults; 3.x's SEP-2260 association check only applies ≥ 2026-07-28 | **[D]** `handler/client.rs:97-190`, `service/client.rs:310-330` |
| MRTR `input_required`, task-augmented results | n/a | only from 2026-07-28 servers; a `CreateTaskResult` would map to `UnexpectedResponse → ServerInternal` (retriable) | **[D]** `service/client.rs:1367-1386,1954-1993` |
| OAuth | not used (`auth` feature off) | not used | **[C]** `Cargo.toml` |

**Will declared and common servers still work?** `config/mcp.toml` declares **no active servers** (every block is a commented
example: npx filesystem/github over stdio, two HTTP examples) **[C]**. Any server that follows the spec's version negotiation
(answer the requested version if supported, else its own latest) keeps working, because the client accepts whatever version comes
back **[M]**. The official TypeScript and Python SDK servers negotiate that way **[Docs, not verified here]**. A server that
*rejects* an unknown `2025-11-25` with an error instead of counter-offering would now fail to hand-shake; I found no such server,
but could not test real third-party servers (the npx filesystem e2e needs a network download, which I did not do).

## 5. Dependencies

**[M]** `cargo tree` (`logs/rmcp-40-*`):

* rmcp 3.4.1 direct deps resolve to the workspace's single versions: `reqwest 0.13.4` (no duplicate), `tokio 1.53.1`, `serde 1.0.229`,
  `serde_json 1.0.151`, `http 1.5.0`, `schemars 1.2.2`, `chrono 0.4.45`; hyper is not pulled by rmcp (its `hyper` dep is behind the unused
  unix-socket feature) **[D]** index.
* `openalpaca_mcp`'s closure: 127 → 129 crates. New duplicates in the workspace: `base64 0.23.1` (beside 0.22.1) and the `darling 0.24.1`
  family (beside 0.20/0.23; `rmcp-macros` moved to `syn 3`, already in the lock). Gone: `iana-time-zone` from this closure.
* `schemars 1.x` is pulled **today and after** only because we take rmcp's **default features** (`macros`, `server`, `base64`), none of
  which a client uses. **Optional, measured:** `default-features = false` compiles mcp/core/daemon clean and passes 34 tests
  (`logs/rmcp-41-*`), cuts the closure to 114 crates, and removes `schemars 1.2.2`, `schemars_derive 1.2.2`,
  `serde_derive_internals 0.30`, `rmcp-macros`, `darling 0.24`, `pastey`, `uuid` from this path; net workspace duplicates −6/+1.
  Incremental patch: `SP/patches/rmcp-optional-no-default-features.diff`. It is a scope change, not forced by the bump — an owner call.
* MSRV: 3.4.1 declares 1.88 **[D]**; builds on 1.93.0 and 1.98.1 **[M]**.

## 6. Risks: persisted data, secrets, wire formats, what users see

* **Persisted data — none.** No rmcp type is stored: `ToolAnnotations` feeds `permission_tier`/capabilities and the `requires_confirmation`
  flag of `GET /v1/tools`, never serialised **[C]** `apps/openalpacad/src/routes/tools.rs:106-120`, no storage hit for `annotations`/`input_schema`
  in `crates/openalpaca_storage` **[C]**. `config_fingerprint` hashes our own `McpServerConfig`, not rmcp types **[C]** `tools/mcp/fingerprint.rs`.
* **Secrets — handling unchanged by the bump.** Bearer/API-key/extra headers are still set as our reqwest client's default headers
  **[C]** `transport/http.rs:57-104`. **Pre-existing leak, measured on both 0.16 and 3.4.1:** a server answering `307` to another origin
  receives `X-API-Key` and every `extra_headers` value (reqwest strips only `Authorization`) **[M]** `probe/redirect-016.txt`, `redirect-341.txt`.
  rmcp's fix for the same class (CVE-2026-64684, fixed in 2.1.0 **[Docs]**) sets `redirect::Policy::none()` on **rmcp's own** default
  client only **[D]** `reqwest/streamable_http_client.rs:398-403`; we pass our own client, so the bump does not fix it. Fix is one line
  (`.redirect(reqwest::redirect::Policy::none())` in `transport/http.rs`) — recommended as its own commit, outside this bump.
* **Wire to MCP servers — changes** (§4): protocol version offered, `MCP-Protocol-Version` header, `ping` params, `clientInfo.version`.
* **Wire to the LLM — unchanged**: the tool `parameters` JSON is byte-identical **[M]**; tool names unchanged **[C]** `bridge.rs:32-33`.
* **What users see**: (a) a tool returning empty content now works instead of respawning the server in a loop (and, once the ladder
  gives up, flipping the row to `Failed{Crashed}`); (b) the failure text for a header-less 401 changes; (c) an expired HTTP session
  now recovers silently. The extension row's `version` still comes from `serverInfo.version`, always present via `initialize` **[C]**
  `managers/mcp.rs:1150`.
* **Pre-existing reconnect-ladder defect (not caused, not fixed):** `install_service` resets `attempt_counter` to 0 on every successful
  handshake **[C]** `client.rs:325`, so an error that is retriable and deterministic (the empty-content case on 0.16; a tool that always
  exceeds `request_timeout`; a `CreateTaskResult` on 3.x) makes `call_tool` loop for ever — 32 cycles in 4 s measured on 0.16 **[M]**.

## 7. Mechanical vs judgement fixes

Mechanical: the six `Implementation` literals; the `CallToolRequestParams` literal (drops `task: None`); the test `InitializeResult`;
18 `ToolAnnotations` literals rewritten by `SP/rmcp-research/fix_annotations.py`, which maps **by field name** (builder chain when every
value is a literal `Some(bool)`, `from_raw(..)` with a per-argument field comment for the two pass-through sites) so no positional swap is
possible; `RawContent::X` → `ContentBlock::X`; `Content::text` → `ContentBlock::text`; e2e `c.raw` match → `c.as_text()`.

Judgement (small, reviewable):
1. `peer_info.server_info` is now `Option`: leave the `server_info` `OnceCell` unset when absent and log `"unknown"` (legacy `initialize`
   always fills it) — `client.rs:260-282`.
2. `ContentBlock` is `#[non_exhaustive]`: `content_kind_name` gains `_ => "unknown"`, so a future block kind still reaches the model as a placeholder.
3. `openalpaca_mcp`'s re-exports replace `Content, RawContent` with `ContentBlock` (no compatibility alias).
4. Comments citing `rmcp-0.16.0` paths/line numbers updated to 3.4.1 (`error.rs`, `client.rs` ping note).

## 8. Proposed commits (in order)

Each commit's patch exists: stage patches are cumulative from HEAD (`SP/patches/rmcp-stage1-0.17.diff` …
`rmcp-stage4-3.4.diff`, the last identical to `rmcp.diff`); a commit's content is the difference between consecutive stages.

1. **`chore(deps): rmcp 0.16 → 0.17`** — `crates/openalpaca_mcp/Cargo.toml` requirement + `Cargo.lock` (rmcp, rmcp-macros). No code.
   Verify: `check -p openalpaca_mcp --all-targets --features test-utils`, `check -p openalpaca_core --all-targets`, `check -p openalpacad --all-targets`,
   `test -p openalpaca_mcp --features test-utils`, `test -p openalpacad managers::mcp`.
2. **`chore(deps): rmcp 0.17 → 1.8 — build its non-exhaustive model types through constructors`** — `Implementation::new`,
   `CallToolRequestParams::new(..).with_arguments(..)`, `InitializeResult::new(..)` in tests, `ToolAnnotations` builders (core + daemon test).
   Verify: as 1, plus `test -p openalpaca_core --lib` (permission tier / sandbox / annotation tests) and `test -p openalpacad routes::tools`.
3. **`chore(deps): rmcp 1.8 → 2.2 — ContentBlock replaces Content/RawContent`** — `lib.rs` re-export, `bridge.rs`, e2e test.
   Verify: as 1 plus `test -p openalpaca_core --lib tools::mcp`.
4. **`chore(deps): rmcp 2.2 → 3.4.1 — server identity is optional`** — `client.rs` handshake, doc citations; lock brings process-wrap 10,
   base64 0.23, darling 0.24. Verify: as 1 + 2, plus `check` on `+1.93.0` for `openalpaca_mcp` (MSRV 1.88), plus the manual e2e
   `cargo test -p openalpaca_mcp -- --ignored` on a machine where npx may fetch `@modelcontextprotocol/server-filesystem`.
   After the daemon tests, list `sleep 3600` orphans with ppid 1 started during the run and kill only those.
5. *(optional, owner decision)* **`chore(deps): take rmcp without default features`** — `SP/patches/rmcp-optional-no-default-features.diff`.
   Verify: as 1.

Follow-ups **outside** the bump (separate commits, each needing a test first): refuse redirects on the MCP HTTP client (§6);
bound the reconnect ladder so a successful re-handshake does not reset the budget for the same failing call (§6); decide whether the
configured `client_info` should actually be sent (override `NotifyingHandler::get_info`) — this changes what servers see, so it is
an owner decision; optionally disable rmcp's client response cache to keep 0.16's no-cache behaviour.

## 9. Open questions

* Does any MCP server the owner actually runs reject protocol `2025-11-25` outright instead of negotiating down? Not testable here.
* Should the bump land as four commits (bisectable) or one? The code is ~290 lines either way; four commits cost three extra lock churns.
* Take `default-features = false` now (drops schemars 1.x and darling 0.24 from the tree) or later?
* The two npx-based e2e tests were not run (they download a package from npm).

Sources consulted **[Docs]**: rmcp `CHANGELOG.md` in `rmcp-3.4.1`; [Migrating to 3.0.0 — rust-sdk discussion #969](https://github.com/modelcontextprotocol/rust-sdk/discussions/969);
[CVE-2026-64684 advisory](https://advisories.gitlab.com/cargo/rmcp/CVE-2026-64684/); rmcp 2.x ContentBlock migration notes in downstream PRs
(e.g. [CertaMesh/gaze#616](https://github.com/CertaMesh/gaze/pull/616)).
