# Group "web": HTTP, WebSocket, CLI line editing, connector stacks

Researcher report, 2026-09-25. Worktree `SP/wt-web` (detached at `5e5ac317`), target dir `SP/target-198`.
`SP` = `/private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/4b1ee495-f087-45a9-9fa8-4325993552c8/scratchpad`.
Patch: `SP/patches/web.diff` (Cargo.toml + Cargo.lock only, 476 lines). **It compiles** on 1.98.1 (whole workspace, all targets, GUI excluded) and on 1.93.0 (CLI + daemon, all targets), with **zero source changes**.

Evidence tags: **measured** (I ran it; command and result given), **read-in-our-code** (file:line at `5e5ac317`), **read-in-dependency-source** (path under `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`, abbreviated `REG/`), **inferred-from-docs** (URL / changelog). Where I could not establish something, it says so.

---

## 1. Bottom line

| Package | From → To | Code changes forced | Result | Risk | Recommendation |
|---|---|---|---|---|---|
| tower-http | 0.6.11 → 0.7.1 | none | compiles; one wire change: CORS responses lose their `Vary` header (measured) | low | upgrade-now |
| tokio-tungstenite (+ tungstenite) | 0.28.0 → 0.30.0 | none | compiles; the Close(1001, "daemon_shutting_down") test passes against axum's 0.29 server | low | upgrade-now (or pin 0.29 to dedupe — owner's call, §5) |
| rustyline | 17.0.2 → 18.0.1 | none | compiles; 8/8 completer tests pass; history file format unchanged; signal-handler lifetime changed (needs a manual TTY check) | low–medium | upgrade-with-care |
| axum 0.8.9, axum-core 0.5.6, tower 0.5.3, hyper 1.11.1, h2 0.4.19, http 1.5.0, tokio 1.53.1, teloxide 0.17.0, twilight-* 0.17.1, tokio-websockets 0.13.3 | already latest stable | — | measured on crates.io API | — | nothing to do |
| reqwest 0.13.4, rustls 0.23.43, hyper-util 0.1.20, hyper-rustls 0.27.9, tokio-rustls 0.26.4, rustls-platform-verifier 0.7.0 | lock refresh only → 0.13.5 / 0.23.45 / 0.1.21 / 0.27.10 / 0.26.5 / 0.7.1 | — | appear in `SP/cargo-update-dryrun.txt`; not bumped in my patch (the lock-refresh group owns them) | low | lock-refresh |

What cannot be done today: **axum 0.9, teloxide 0.18 and twilight 0.18 do not exist, not even as pre-releases** (measured, §4). The one real dependency-hygiene win in this group is not a version bump but a feature flag: **teloxide `default-features = false` + `rustls-native-roots` removes native-tls/OpenSSL (8 crates) from the daemon** (measured, compiles, §6) — an owner decision because it changes the Telegram connector's TLS stack.

---

## 2. What I ran

All cargo commands: `nice -n 10 env CARGO_TERM_COLOR=never CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=SP/target-198 cargo +<tc> …`, output redirected to `SP/logs/`, counted with `grep -c '^warning' / '^error'`.

| # | Command | Log | Result |
|---|---|---|---|
| 1 | edit `Cargo.toml` (3 lines) then `cargo +1.98.1 update -p tower-http -p tokio-tungstenite@0.28.0 -p rustyline` | `web-update.txt` | 12 lock changes (list in §7); resolver stayed MSRV-aware at 1.93 |
| 2 | `cargo +1.98.1 check --workspace --all-targets --exclude openalpaca_gui` | `web-check-1.txt` | exit 0, **0 errors, 1 warning** (the pre-existing future-incompat note for `proc-macro-error2 v2.0.1`, not from this group), 2m58s |
| 3 | `cargo +1.98.1 test -p openalpacad -- --list events chat graceful_shutdown cors` | `web-test-daemon-list.txt` | 71 tests match: 41 `routes::chat`, 9 `events::persistence`, 8 `routes::events_history`, 6 `routes::chat_types`, 2 `routes::events`, 2 `graceful_shutdown_tests`, 2 `connector_bridge`, 1 `event_bridge`. **No test matches "cors" — the daemon has no CORS test at all.** |
| 4 | `cargo +1.98.1 test -p openalpacad -- events chat graceful_shutdown cors` | `web-test-daemon.txt` | **71 passed, 0 failed** (511 filtered out); both `graceful_shutdown_tests` pass |
| 5 | `cargo +1.98.1 test -p openalpaca` | `web-test-cli.txt` | **249 passed, 0 failed, 1 ignored** (`test_util::singleton_lock_holder_process`, ignored by design); all 8 `repl::completer::tests` pass |
| 6 | `cargo +1.93.0 check -p openalpaca -p openalpacad --all-targets` | `web-check-193.txt` | exit 0, **0 errors, 0 warnings**, 1m06s |
| 7 | CORS probe: standalone crate `SP/scratch-web-cors-probe` (axum 0.8.9 + `CorsLayer::permissive()`, run once per tower-http version, lock seeded from the worktree's) | `web-cors-probe-th06.txt`, `web-cors-probe-th07.txt` | only difference: `Vary` gone (§3.1) |
| 8 | teloxide experiment in the worktree, then reverted: `default-features = false, features = ["macros", "ctrlc_handler", "rustls-native-roots"]`, `cargo update -p teloxide --offline`, `cargo +1.98.1 check -p openalpacad --all-targets`, `cargo +1.98.1 test -p openalpaca_connectors --features telegram,discord` | `web-teloxide-exp-*.txt` | 8 crates removed; check exit 0, 0 errors; connectors 39 passed |
| 9 | teloxide client probe `SP/scratch-web-teloxide-probe`: `teloxide::Bot::new(..)` under that feature set, no request sent | `web-teloxide-probe.txt` | exit 0 — the reqwest 0.12 rustls client builds (no panic) on macOS |
| 10 | tokio-tungstenite 0.29 option in a scratch copy `SP/scratch-web-teloxide` (lock only): `cargo update -p tokio-tungstenite@0.30.0 --precise 0.29.0 --offline` | `web-tt029-update.txt` | removes 8 packages versus 0.30 (§5) |
| 11 | `cargo tree -i …` inverse trees before/after; duplicate-version census of both lockfiles | `web-tree-*.txt`, `web-dups-before.txt`, `web-dups-after.txt` | §8 |

After step 8 the worktree's `Cargo.toml`/`Cargo.lock` were restored from copies and `git diff` was compared byte-for-byte with `SP/patches/web.diff`: identical (measured, `cmp`).

### 2a. Rust 1.93.0

`cargo +1.93.0 check -p openalpaca -p openalpacad --all-targets` — **measured: exit 0, 0 errors, 0 warnings** (`web-check-193.txt`). (Only the two binaries that use these crates were checked on 1.93.0, to keep load down; the whole workspace was checked on 1.98.1.) The resolver picked every version MSRV-aware at 1.93 (step 1's output says "Locking 12 packages to latest Rust 1.93 compatible versions"), and none of the three bumps declares an MSRV above 1.85 (crates.io API: tower-http 0.7.1 → 1.65, tokio-tungstenite/tungstenite 0.30.0 → 1.85; rustyline declares none). **rustyline 18 needs `std::fs::File::lock`, stabilised in Rust 1.89** (read-in-dependency-source: `REG/rustyline-18.0.1/src/history.rs`, `file.lock()?` / `file.lock_shared()?` replaced `fd_lock::RwLock`) — satisfied by the workspace's 1.93 floor, but it means rustyline 18's real MSRV is ≥ 1.89 although it declares none. So these three bumps do **not** depend on the toolchain bump and may land before or after it.

---

## 3. Per-package findings

### 3.1 tower-http 0.6.11 → 0.7.1 (daemon CORS)

**Our usage** (read-in-our-code): exactly one call site — `apps/openalpacad/src/router.rs:9` (`use tower_http::cors::CorsLayer`) and `:477` (`.layer(CorsLayer::permissive())` on the merged router). The workspace enables only the `cors` feature (`Cargo.toml:64`). No other tower-http layer is used; `DefaultBodyLimit` (`router.rs:153`) is axum's own. Nothing else in `apps/` or `crates/` imports tower or tower-http (grep).

**Breaking changes in 0.7.0/0.7.1** (read-in-dependency-source `REG/tower-http-0.7.1/CHANGELOG.md`): compression `*`/`identity;q=0` handling and `SizeAbove` u64, removal of the no-op `tokio`/`async-compression` features, gRPC classify `#[non_exhaustive]`, follow-redirect extension forwarding, `ServeDir` trailing-slash 404, and **"`cors`: relax the `Vary` header defaults (#674)"**. Only the last touches a feature we enable. MSRV 1.64 → 1.65.

**Source-level change** (read-in-dependency-source: `diff -ru REG/tower-http-0.6.11/src/cors REG/tower-http-0.7.1/src/cors`): `CorsLayer` gained `is_vary_custom` and `update_vary_header()`, called from `Layer::layer`. Unless `.vary()` is set explicitly, `Vary` is now derived: `Origin` only when the allowed origin depends on the request, `Access-Control-Request-Method`/`-Headers` only when those are mirrored. `permissive()` is `allow_headers(Any).allow_methods(Any).allow_origin(Any).expose_headers(Any)` (unchanged, `cors/mod.rs:133`), all constants, so **it now emits no `Vary` at all**; upstream's own test is `permissive_vary_header_is_empty`. (Aside: the `Cors::permissive(inner)` constructor, `mod.rs:564`, does not call `update_vary_header` and keeps the old default — we do not use it.)

**Measured** with the probe (step 7), axum 0.8.9 + `CorsLayer::permissive()`:

| Request | 0.6.11 | 0.7.1 |
|---|---|---|
| `GET /v1/health`, `Origin: tauri://localhost` | `access-control-allow-origin: *`, `access-control-expose-headers: *`, `vary: origin, access-control-request-method, access-control-request-headers` | same **without `vary`** |
| `OPTIONS /v1/chat` preflight (`Origin: http://localhost:1420`, `ACR-Method: POST`, `ACR-Headers: authorization, content-type, x-workspace-path`) | `access-control-allow-headers: *`, `-methods: *`, `-origin: *`, `vary: …` | same **without `vary`** |
| `GET` with no `Origin` | as row 1 | as row 1 without `vary` |

Every `Access-Control-*` header is byte-identical across the two versions (diff of the two logs: only the three `vary` lines).

**Does it matter?** The GUI depends on this CORS layer: its fetches send `Authorization: Bearer …` (read-in-our-code `apps/openalpaca-gui/src/lib/http.ts:147`), `If-Match` on memory/persona writes (`lib/api/memories.ts:303`, `lib/api/persona.ts:70`), and it reads the `ETag` response header (`lib/api/persona.ts:59`, which falls back to hashing the body if the header is missing). All of that rides on `allow-headers`/`expose-headers`/`allow-origin`, which do not change. `Vary` only matters to a cache that could serve one origin's response to another; since `access-control-allow-origin` is the constant `*`, the responses are the same for every origin, so dropping `Vary` cannot make a cache serve a wrong answer. The daemon sets no `Cache-Control` anywhere (grep of `apps/openalpacad/src`). Conclusion (inferred from the above, not measured in a webview): no user-visible effect.

**axum 0.8.9 compatibility** (read-in-dependency-source + measured): tower-http 0.7.1 depends on `http ^1.0`, `http-body ^1`, `tower-layer ^0.3.3`, `tower-service ^0.3`, `tower ^0.5` (optional) — the same single versions axum 0.8.9 uses (`http 1.5.0`, `http-body 1.1.0`, `tower-layer 0.3.3`, `tower-service 0.3.3`, `tower 0.5.3` in the lock). The workspace check compiles `router.rs` unchanged. axum's own tower-http 0.6 dependency is optional and not enabled in our tree (`cargo tree -i tower-http@0.6.11` shows only reqwest parents).

**New duplicate**: reqwest 0.12.28 and 0.13.x (latest 0.13.5 too — crates.io API) require `tower-http ^0.6.8` with `follow-redirect` (read-in-dependency-source `REG/reqwest-0.13.4/Cargo.toml:435-438`). So after the bump **tower-http 0.6.11 stays in the tree and both versions link into `openalpacad`** (measured: `cargo tree -p openalpacad -e normal -i tower-http@0.6.11` and `@0.7.1` both non-empty). The CLI links only 0.6.11. Small crate, feature-gated to `cors` / `follow-redirect`: compile-time and a few KB, no behaviour. Upstream (reqwest) to fix.

### 3.2 tokio-tungstenite 0.28.0 → 0.30.0 (tungstenite 0.28 → 0.30)

**Our usage** (read-in-our-code): the CLI's `daemon tail` (`apps/openalpaca/src/commands/tail.rs:11,159,180,192`) and `tasks confirmations watch` (`commands/confirmations.rs:41,377,392,417`) — `connect_async(&str)`, `Message::Text(text)` fed to `serde_json::from_str(&text)`, `Message::Close(_)`; the daemon's dev-only `graceful_shutdown_tests.rs:39,111,121,144,158-162` — `Message::Close(Some(close))`, `close.code == CloseCode::Away`, `close.reason.as_str() == "daemon_shutting_down"`. The daemon itself serves WebSockets through axum's own tokio-tungstenite 0.29 (`Cargo.lock`: axum → `tokio-tungstenite 0.29.0`); `tokio-tungstenite` is only a **dev**-dependency of `openalpacad` (`apps/openalpacad/Cargo.toml:59`).

**API changes 0.28 → 0.30** (read-in-dependency-source):
- tokio-tungstenite: `diff -ru REG/tokio-tungstenite-0.28.0/src REG/tokio-tungstenite-0.30.0/src` — a doc comment on cancel-safety only. `Cargo.toml` differs only in version, MSRV (1.63 → 1.85) and the tungstenite requirement; feature list unchanged.
- tungstenite (`REG/tungstenite-0.30.0/CHANGELOG.md` + source diff): 0.29 writes client request headers as raw bytes (non-visible ASCII allowed); 0.30 **rejects a malformed `Sec-WebSocket-Key` on the server side** (new `ProtocolError::InvalidSecWebSocketKey`), moves to `rand 0.10` and `sha1 0.11`, MSRV 1.85, and compares frame length in u64 before narrowing. `Message`, `Utf8Bytes`, `Bytes`, `CloseFrame { code, reason: Utf8Bytes }`, `CloseCode::Away`, `connect_async` are unchanged since 0.26 (the `Utf8Bytes`/`Bytes` switch was 0.26 — already absorbed).
- The server-side key check lives in tungstenite 0.30, which only our *client* uses; the daemon's server stays on axum's 0.29 until axum moves.

**Measured**: compiles unchanged; `graceful_shutdown_completes_with_a_live_events_socket` passes — a tokio-tungstenite **0.30 client** reads the `daemon_shutting_down` text frame and then `Close(Some(CloseFrame { code: Away, reason: "daemon_shutting_down" }))` from the axum/tungstenite **0.29 server**. Not measured: the CLI's `tail`/`confirmations watch` loops against a live daemon (no test covers them; they use only `Message::Text`/`Message::Close(_)`, the same types the test exercises).

**Duplication** (measured): the tree already carried two tungstenites before the bump (ours 0.28, axum's 0.29); after it, 0.29 (axum) + 0.30 (ours). No *binary* links both: the CLI links only 0.30, the daemon only 0.29 (`cargo tree -p <bin> -e normal --target aarch64-apple-darwin -i tungstenite@…`). The cost is compile time for `--workspace` builds and tests, plus the RustCrypto 0.11 generation that tungstenite 0.30 brings (`sha1 0.11.0`, `digest 0.11.3`, `block-buffer 0.12.1`, `crypto-common 0.2.2`, `hybrid-array 0.4.15`, `const-oid 0.10.2` — 6 new crates, all in the CLI). If another group bumps `sha2` to 0.11 (listed in `direct-deps.tsv`), that generation arrives anyway and the marginal cost is `sha1 0.11` alone. Side effect in the other direction: `utf-8 0.7.6` leaves the tree.

### 3.3 rustyline 17.0.2 → 18.0.1 (CLI REPL)

**Our usage** (read-in-our-code): `apps/openalpaca/src/repl/mod.rs:102-144` — `Config::builder().max_history_size(1000)?.auto_add_history(true)`, `Editor::<ReplHelper, FileHistory>::with_config`, `set_helper`, `load_history`/`save_history` on `state/repl_history` (`:14-17`), `readline(">> ")` inside `tokio::task::block_in_place`, matching `ReadlineError::Interrupted` / `Eof` with a catch-all `Err(e) => return Err(e.into())`; `repl/completer/mod.rs` — `Completer` (`Pair`), `Hinter`, empty `Highlighter`/`Validator`/`Helper` impls; tests build `Context::new(&DefaultHistory::new())`.

**API changes** (read-in-dependency-source, `diff -u REG/rustyline-17.0.2/src/{lib,config,completion,hint,highlight,validate,error,history}.rs REG/rustyline-18.0.1/…`; release notes inferred-from-docs https://github.com/kkawakam/rustyline/releases/tag/v18.0.0):
- `Editor::readline` / `readline_with_initial` now take `&P where P: Prompt + ?Sized` (new `Prompt` trait, `src/prompt.rs`, implemented for `str`, `String` and `(raw, styled)`); `">> "` still type-checks (measured).
- `Completer`, `Hinter`, `Highlighter`, `Validator`, `Helper`, `Context`, `Config` builder methods, `ReadlineError` variants: no signature change in the files diffed (only `use … as _` renames).
- `Config::color_mode()` now honours `NO_COLOR`. Our helper does no highlighting, so nothing we draw changes.
- `with-file-history` no longer uses `fd-lock`: `FileHistory::save`/`append`/`load` lock with `std::fs::File::lock` / `lock_shared` (flock on Unix, LockFileEx on Windows — the same primitives fd-lock used). `fd-lock 4.0.4` leaves the tree; `nix 0.30.1` leaves too (rustyline 18 uses `nix ^0.31.2`, the version the CLI already uses) — **one duplicate fewer**.
- **Signal handlers are now installed per `readline` call, not per `Editor`** (release note "Install signal handlers only when actually reading #903"; source: 17 installs `Sig::install_sigwinch_handler()` — which `sigaction`s **SIGINT and SIGWINCH** — in `PosixTerminal::new` (`REG/rustyline-17.0.2/src/tty/unix.rs:1388-1392`) and restores them on `PosixTerminal` drop (`:1535-1538`); 18 installs them in `create_reader` (`REG/rustyline-18.0.1/src/tty/unix.rs:1489-1493`) and restores on `TtyIn` drop (`:163-169`), i.e. when `readline` returns).
  Consequence for us (inferred from those two sources plus `repl/mod.rs:106-115` and `chat_stream/mod.rs:413`, **not measured**): under 17, rustyline's SIGINT handler stayed installed for the whole REPL session, including while `execute_chat` streams a turn and waits on `tokio::signal::ctrl_c()`; tokio's handler (signal-hook-registry) then sat on top of it. Under 18, between prompts SIGINT has its prior disposition (tokio's handler once a turn has run). The intent upstream is exactly this; the expected user-visible effect is none or better, but Ctrl+C during a streaming turn and at the next prompt must be checked by hand on a real terminal (§9, proposed commit C3's verification).
- Rendering: "Support minimal repaint", "Fix partial refresh", "Fix page_completions" (18.0.1). Cosmetic; manual check.

**Persisted data — `state/repl_history`** (read-in-dependency-source): `save_to`/`load_from` bodies are identical between 17.0.2 and 18.0.1 apart from `use` renames (`diff` of the two function bodies), the `#V2` header constant is unchanged (`history.rs:465` in 18, `:467` in 17), and the umask/`fix_perm` 0600 handling is unchanged. The file format and its permissions do not change; an existing history loads.

**Measured**: compiles unchanged (1.98.1 workspace; 1.93.0 CLI); `repl::completer::tests` 8/8 pass. Not measured: an interactive session (no TTY here).

---

## 4. Latest-stable confirmation (crates.io API, 2026-09-25, measured with `curl https://crates.io/api/v1/crates/<name>` and `/versions`)

| Crate | Locked | max_stable = max_version | Note |
|---|---|---|---|
| axum | 0.8.9 | 0.8.9 (2026-04-14) | **no 0.9 release or pre-release** |
| axum-core | 0.5.6 | 0.5.6 | |
| tower | 0.5.3 | 0.5.3 | |
| tower-http | 0.6.11 → 0.7.1 | 0.7.1 (2026-08-31) | |
| hyper | 1.11.1 | 1.11.1 | only hyper 1.x in the lock (no 0.14) |
| hyper-util | 0.1.20 | 0.1.21 (2026-09-24, MSRV 1.85) | lock refresh |
| h2 | 0.4.19 | 0.4.19 | |
| http | 1.5.0 | 1.5.0 | single version |
| reqwest | 0.13.4 (+ 0.12.28 via teloxide) | 0.13.5 | lock refresh; teloxide pins 0.12 |
| rustls | 0.23.43 | stable 0.23.45; max 0.24.0-dev.1 (2026-07-23, a dev pre-release) | lock refresh; 0.24 not stable |
| hyper-rustls / tokio-rustls / rustls-platform-verifier | 0.27.9 / 0.26.4 / 0.7.0 | 0.27.10 / 0.26.5 / 0.7.1 | lock refresh |
| tokio | 1.53.1 | 1.53.1 | |
| tokio-tungstenite / tungstenite | 0.28.0 → 0.30.0 | 0.30.0 (2026-07-11) | |
| tokio-websockets (twilight-gateway) | 0.13.3 | 0.13.3 | |
| rustyline | 17.0.2 → 18.0.1 | 18.0.1 (2026-06-24) | |
| teloxide / teloxide-core | 0.17.0 / 0.13.0 | 0.17.0 / 0.13.0 (2025-07-11) | **no 0.18 release or pre-release** |
| twilight-gateway/-http/-model | 0.17.1 | 0.17.1 (2025-12-13) | **no 0.18 release or pre-release** |

What the next majors would bring (inferred-from-docs; read on each project's default branch through `gh api`, 2026-09-25):
- **axum 0.9** (tokio-rs/axum `main`, `axum/CHANGELOG.md` "Unreleased"; the `0.9` milestone has 4 open / 11 closed issues, no due date; `axum/Cargo.toml` on `main` is still `version = "0.8.9"` and still requires `tokio-tungstenite 0.29` and optional `tower-http 0.6.8`). Breaking items that would touch us: `axum::serve` applies hyper's default `header_read_timeout`, and its future's output loses `io::Result` (our `main.rs` serve/`with_graceful_shutdown` call sites and `graceful_shutdown_tests.rs`'s `serve` helper would need adjusting); nested-router fallback merging; `serve` gains a generic body parameter. Additions of interest: `ListenerExt::limit_connections`, `Serve::connection_lifetime_limits`, `sse::Event::raw`. Nothing to plan until it is published.
- **teloxide 0.18** (teloxide/teloxide `master`, `CHANGELOG.md` "unreleased"; crate manifests still say teloxide 0.17.0 / teloxide-core 0.13.0): **teloxide-core moves to `reqwest 0.13.3`** (`crates/teloxide-core/Cargo.toml` on master), which would remove the reqwest 0.12 / tower-http 0.6-via-0.12 duplicate stack; TBA 9.2 support; several [BC] type changes (`ChatFullInfoPublicKind::Supergroup` boxed, `delete_chat_photo` → `True`, `upload_sticker_file` → `File`, command-separator parsing now a compile error when misused). Whether any of those reach our `telegram/` code is not established — nothing to compile against yet.
- **twilight 0.18**: no sign — `main` and `next` both carry `twilight-http` 0.17.1, workspace `rust-version = "1.89"`; milestones exist (Builder Restructuring, Model Rework, …) with no dates.

---

## 5. Owner choice: tokio-tungstenite 0.30 (latest) or 0.29 (axum's)

Measured in a scratch copy (step 10): pinning our requirement to `0.29.0` gives one tungstenite in the whole tree and removes 8 packages compared with the 0.30 patch (`block-buffer 0.12.1`, `const-oid 0.10.2`, `crypto-common 0.2.2`, `digest 0.11.3`, `hybrid-array 0.4.15`, `sha1 0.11.0`, `tokio-tungstenite 0.30.0`, `tungstenite 0.30.0`). Our code needs nothing either way (0.29 sits between 0.28 and 0.30, whose APIs we use unchanged). Trade: 0.30 is "latest stable" and carries the frame-length/`Sec-WebSocket-Key` hardening (which matters on a server; ours is a client); 0.29 is dedupe now and must be re-bumped when axum moves. My recommendation is **0.30** as the brief's goal is latest-stable and no binary links both (§3.2); the 0.29 pin is the fallback if workspace compile time matters more.

---

## 6. Duplicates in the network stack: who pulls them, and what a flag removes

**Correction to the brief's premise** (measured, both lockfiles): at HEAD `hyper`, `h2`, `hyper-util`, `hyper-rustls`, `hyper-tls`, `native-tls`, `dashmap`, `dialoguer` and `notify` each appear in **one** version only — they are not duplicated. What *is* duplicated in this group's area:

| Duplicate (after the patch) | Older copy pulled by | Ours to fix? |
|---|---|---|
| `reqwest 0.12.28` / `0.13.4` | `teloxide-core 0.13.0` (`reqwest ^0.12.7`) | upstream; teloxide master already on reqwest 0.13 — goes when teloxide publishes |
| `tower-http 0.6.11` / `0.7.1` (**new**) | `reqwest 0.12.28` and `0.13.x` (`tower-http ^0.6.8`, follow-redirect) | upstream (reqwest) |
| `tungstenite`/`tokio-tungstenite 0.29.0` / `0.30.0` (was 0.28/0.29) | `axum 0.8.9` (`tokio-tungstenite ^0.29.0`) | ours by choice — §5 |
| `rand 0.9.5`, `rand_core 0.9.5`, part of `getrandom 0.3.4` | `tungstenite 0.29.0` ← axum | upstream (axum) |
| `sha1`/`digest`/`block-buffer`/`crypto-common` 0.10-gen vs 0.11-gen (**new**) | 0.10-gen: axum (`sha1 ^0.10`), aes-gcm 0.10, sha2 0.10 …; 0.11-gen: tungstenite 0.30 | mixed — shrinks if the crypto group moves sha2/aes-gcm to the 0.11 generation, or with the 0.29 pin |
| `nix 0.30.1` / `0.31.3` (**removed** by rustyline 18) | was rustyline 17 | fixed by this patch |
| `foreign-types 0.3.2`, `native-tls`, `openssl`, `openssl-sys`, `hyper-tls`, `tokio-native-tls` (single versions, but a second TLS stack) | `teloxide` **default** features → `teloxide-core/native-tls` → `reqwest 0.12/native-tls` | **ours** — a feature flag, below |
| `security-framework 2.11.1` / `3.x`, `core-foundation 0.9.4` / `0.10.1` | `keyring 3.6.3` (not native-tls) | secrets group (keyring 4) |

`cargo tree -i hyper-tls|native-tls|openssl -e normal --target all` (measured, `web-tree-baseline.txt`): the **only** path to native-tls/OpenSSL is `teloxide 0.17.0 → teloxide-core 0.13.0 → reqwest 0.12.28`. twilight does not pull it — `twilight-http 0.17.1` uses `hyper 1` + `hyper-rustls` (default features `decompression`, `rustls-platform-verifier`), `twilight-gateway` uses `tokio-websockets` (crates.io dependency API). No crate pulls hyper 0.14. On Linux, `openssl-sys` is built with its `default` feature only — **not vendored** (measured, `cargo tree -p openalpacad -e features -i openssl-sys --target x86_64-unknown-linux-gnu`), so a Linux build of the daemon needs the system OpenSSL headers and links libssl at run time. On macOS the same path is `native-tls → security-framework` and OpenSSL is absent (measured, `--target aarch64-apple-darwin`: "nothing to print" for openssl).

**The flag** (measured, step 8/9): `teloxide = { version = "0.17.0", default-features = false, features = ["macros", "ctrlc_handler", "rustls-native-roots"] }` removes `native-tls`, `openssl`, `openssl-sys`, `openssl-macros`, `hyper-tls`, `tokio-native-tls`, `foreign-types 0.3.2`, `foreign-types-shared 0.1.1`, adds nothing (`rustls-native-certs 0.8.4` and `hyper-rustls` are already in the tree), `cargo check -p openalpacad --all-targets` passes, connector tests 39/39, and `teloxide::Bot::new` builds its client on macOS. `teloxide` features (read-in-dependency-source `REG/teloxide-0.17.0/Cargo.toml`): `default = ["native-tls", "ctrlc_handler", "teloxide-core/default"]`, `rustls-native-roots = ["teloxide-core/rustls-native-roots"]` → `reqwest/rustls-tls-native-roots` = `rustls-native-certs` + `__rustls-ring` (`REG/reqwest-0.12.28/Cargo.toml` features). `ctrlc_handler` only enables `tokio/signal`, which we already have; we never call `enable_ctrlc_handler` (read-in-our-code grep; the dispatcher is built at `crates/openalpaca_connectors/src/telegram/connector.rs:98`).
Why it is a decision, not a mechanical fix: it changes how the Telegram connector verifies `api.telegram.org`'s certificate — from the OS TLS library (Security.framework / SChannel / OpenSSL with the OS store) to rustls with the OS root store loaded by `rustls-native-certs` (the `rustls` feature would instead bundle Mozilla's webpki roots, which ignores OS-installed corporate CAs). Not measured: a live Telegram handshake (I did not send any request to Telegram). It is the same trust model the rest of the daemon's HTTP already uses via reqwest 0.13 (`rustls` + `rustls-platform-verifier`), so the recommendation is to adopt it, as its own commit, verified by a live bot start.

Also noted, not changed: rustls is compiled with **both** `ring` (our `openalpaca_connectors/Cargo.toml` `rustls = { features = ["ring"] }` for the Discord connector, which installs ring as the process default at `crates/openalpaca_connectors/src/discord/connector.rs:160-162`) and `aws-lc-rs` (reqwest 0.13's `rustls` feature) — measured `cargo tree -e features -i rustls`. Two crypto backends in one binary; pre-existing; out of this group's scope.

---

## 7. The patch (`SP/patches/web.diff`)

`Cargo.toml` (3 lines): `tower-http = { version = "0.7.1", features = ["cors"] }`, `tokio-tungstenite = "0.30.0"`, `rustyline = "18"`.
`Cargo.lock` (measured, `cargo update` output): Added `tower-http 0.7.1`, `sha1 0.11.0`, `digest 0.11.3`, `block-buffer 0.12.1`, `crypto-common 0.2.2`, `hybrid-array 0.4.15`, `const-oid 0.10.2`; Updated `rustyline 17.0.2 → 18.0.1`, `tokio-tungstenite 0.28.0 → 0.30.0`, `tungstenite 0.28.0 → 0.30.0`, `radix_trie 0.2.1 → 0.3.0`, `endian-type 0.1.2 → 0.2.0`; Removed `fd-lock 4.0.4`, `nix 0.30.1`, `utf-8 0.7.6`. Package count 732 → 736; names with more than one version 65 → 69 (+block-buffer, crypto-common, digest, sha1, tower-http; −nix).
**No `.rs` file changes. Mechanical fixes: none needed. Judgement fixes: none.** Optional cosmetic: `apps/openalpaca/src/repl/completer/mod.rs:1` says "for rustyline v17" — a doc comment to bump in the same commit.

---

## 8. Risks

- **Persisted data**: the only on-disk artefact touched is `state/repl_history` — format, header and permissions unchanged (read-in-dependency-source, §3.3); an existing file keeps loading. No database, config or memory file is involved. Concurrent REPLs: locking primitive changes from fd-lock to std `File::lock`, both flock on Unix (read-in-dependency-source).
- **Secrets**: none of the three touches key storage. The daemon token travels in the CLI's WebSocket URL query (`tail.rs:150-154`) exactly as before; tungstenite 0.29's raw-bytes header writing does not change a URL.
- **Wire formats**: HTTP responses lose `Vary: origin, access-control-request-method, access-control-request-headers` (measured). All `Access-Control-*` headers unchanged (measured). WebSocket frames and close codes unchanged (measured by the shutdown test across 0.30 client / 0.29 server). The daemon's WebSocket server is not touched by this group.
- **User-visible behaviour**: GUI — none expected (CORS headers it relies on unchanged; inferred, not measured in a webview). CLI REPL — signal-handler lifetime and repaint logic changed (source-read; manual TTY check required). `NO_COLOR` now also affects rustyline (our helper draws no colour, so nothing changes).
- **Build**: Linux builds still need system OpenSSL until the teloxide flag lands (pre-existing). rustyline 18's undeclared MSRV is ≥ 1.89 (satisfied).
- **Test gap**: the daemon has no CORS test and the CLI has no test of its WebSocket loops. Proposed commit C0 closes the first.

---

## 9. Proposed commits, in order

- **C0 (optional, before the bump) `test(daemon): pin the CORS headers the GUI relies on`** — move the layer into `fn cors_layer() -> CorsLayer` in `router.rs` and add a test that drives a trivial `Router` through `tower::ServiceExt::oneshot` (as in `SP/scratch-web-cors-probe/src/main.rs`), asserting `access-control-allow-origin: *`, `-allow-headers: *`, `-allow-methods: *`, `-expose-headers: *` on a GET-with-Origin and a preflight; deliberately *not* asserting `Vary`. Verification: passes on tower-http 0.6.11, then unchanged on 0.7.1. (Needs `tower` with `util` as a dev-dependency of `openalpacad`; not in my patch because the bump does not force it.)
- **C1 `chore(deps): tower-http 0.7`** — `Cargo.toml:64` + lock. Verification: `cargo check --workspace --all-targets --exclude openalpaca_gui`; `cargo test -p openalpacad` (C0's test, if taken); GUI smoke against a daemon on an isolated store (`OPENALPACA_HOME_STORE` pointing at a scratch dir): load, send a chat, open Settings → Persona (reads `ETag`), save a memory edit (`If-Match`).
- **C2 `chore(deps): tokio-tungstenite 0.30`** (or 0.29 — §5) — `Cargo.toml:67` + lock. Verification: `cargo test -p openalpacad -- graceful_shutdown events`; `cargo test -p openalpaca`; by hand on an isolated store: `openalpaca daemon tail` receives events and prints "Connection closed by server" on `openalpaca daemon stop`; `openalpaca tasks confirmations watch` likewise.
- **C3 `chore(deps): rustyline 18`** — `Cargo.toml:121` + lock (+ the `completer/mod.rs:1` comment). Verification: `cargo test -p openalpaca`; by hand in a real terminal on an isolated store: start the REPL (history from a pre-bump build loads — up-arrow shows it), Tab-complete `/mo`, see the hint, run a turn and press Ctrl+C mid-stream ("(interrupted)", then a working prompt), Ctrl+C at an empty prompt (continues), resize the window while typing, `exit` (history saved; reopen shows it), and one run with `NO_COLOR=1`.
- **C4 (owner decision) `chore(connectors): teloxide over rustls with native roots`** — `Cargo.toml:127` → `teloxide = { version = "0.17.0", default-features = false, features = ["macros", "ctrlc_handler", "rustls-native-roots"] }` + lock (−8 crates, native-tls/OpenSSL gone). Verification: `cargo check -p openalpacad --all-targets`; `cargo test -p openalpaca_connectors --features telegram,discord`; `cargo tree -i native-tls --workspace --target all` prints nothing; a live Telegram bot start and one message round-trip on an isolated store (the one thing not measured here); on Linux, a build with no OpenSSL dev headers installed.

C1–C3 are independent of each other and of the toolchain bump (compile on 1.93.0 and 1.98.1). The lock refresh of reqwest/rustls/hyper-util/hyper-rustls/tokio-rustls belongs to the lock-refresh group and does not interact with these three.

---

## 10. Open questions

1. Ctrl+C semantics in the REPL under rustyline 18 (§3.3) — needs a human at a TTY.
2. tokio-tungstenite 0.30 vs 0.29 (§5).
3. Adopt the teloxide rustls flag (§6)? And if so, `rustls-native-roots` (OS store, my recommendation) or `rustls` (bundled webpki roots)?
4. Whether a CORS regression test (C0) is wanted — the daemon has none today.
5. Not established: any effect of the `Vary` removal on WebKit's/WebView2's HTTP cache in the packaged app (reasoned harmless because `Access-Control-Allow-Origin` is `*`, not observed).
