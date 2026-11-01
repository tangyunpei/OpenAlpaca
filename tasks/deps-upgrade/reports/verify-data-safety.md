# Data-safety verification of the bump research (skeptic pass)

Role: data-safety skeptic. I tried to refute the load-bearing claims of the eight group reports, using a different method from the researcher's wherever I could. Date: 2026-09-25.
`SP` = `/private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/4b1ee495-f087-45a9-9fa8-4325993552c8/scratchpad`, `REG` = `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f`.
Scratch projects are in `SP/verify/`, target dirs in `SP/target-verify*`, and logs in `SP/logs/verify/`. **I had no worktree and changed none.** The only cargo commands I ran in another group's worktree were `cargo tree --locked --offline`, which is read-only; `git status` in those worktrees is unchanged. So there is no patch to save. I did not touch `~/.openalpaca`, the keychain, rustup defaults or the main checkout.

Evidence tags: **[measured]** (I ran it), **[our-code]** (file:line), **[dep-src]** (a path under `REG`), **[docs]** (a URL). A verdict is "refuted" only where I have evidence. Where I could not check a claim independently, it says "unverified".

## 1. Verdict summary

| # | Group / claim (short) | Verdict | Method |
|---|---|---|---|
| 1 | toolchain: lock refresh = dry run + takecell; data crates unchanged | confirmed | parsed both Cargo.lock files |
| 2 | toolchain: nothing toolchain-dependent is persisted | confirmed | wider grep of our code |
| 3 | toolchain: 1.98 Debug escape is user-visible; approval escape unaffected | **partially-true** | measured all 1,112,064 scalars on both toolchains |
| 4 | toolchain: Unicode 17 (1.94) cannot orphan files; affects only the index | **partially-true** | measured `char::UNICODE_VERSION` and every char's case and class tables |
| 5 | toolchain: 1.98.0 vtable miscompile, pin 1.98.1 | partially-true (not data) | issue page + our code |
| 6 | toolchain: toml_edit 0.25.13→0.25.15 has no output change | confirmed (on 50 inputs) | differential harness running a verbatim copy of `apply_delta` |
| 7 | secrets: `secret_encrypted` decrypts in both directions | confirmed | third-party oracle (ring) |
| 8 | secrets: `.master_key` format unchanged; the key comes from the OS CSPRNG | confirmed | oracle program + dep-src |
| 9 | secrets: SHA-256 hex unchanged | confirmed | Python hashlib oracle + review of all 10 rewrites |
| 10 | secrets: `{:x}` on sha2 0.11 is a compile error | confirmed (with a caveat) | scratch compile |
| 11 | secrets: base64 0.23 behaves the same | confirmed | own differential, 679,320 checks |
| 12 | secrets: keyring v4 addresses the credentials v3 wrote | confirmed from source (not measured) | independent re-read of 4 crates |
| 13 | secrets: keychain reached only when use_keychain or by the reverse migration | **partially-true** | our-code grep |
| 14 | secrets: rand 0.10.3 / blake3 need nothing | confirmed | registry CHANGELOG + src diff |
| 15 | storage: DB and index portable 3.51 ↔ 3.53, WAL included | confirmed | system SQLite 3.51.0 replays a 3.53.2 WAL |
| 16 | storage: 17-digit REAL→TEXT reaches nothing we store | confirmed | re-measured + wider code search |
| 17 | storage: compile flags identical | confirmed | runtime `PRAGMA compile_options` on both |
| 18 | storage: an older build still reads schema_version after 3.53-only ALTERs | confirmed | two older SQLite readers (3.51.0, 3.50.6) |
| 19 | storage: the WAL-reset fix applies to us | confirmed | docs re-read + our code |
| 20 | storage: rusqlite transaction semantics unchanged | confirmed | measured on both versions |
| 21 | documents: the nested-array PDF aborts HEAD's daemon | confirmed | dep-src (lopdf parser recursion) + our runtime config |
| 22 | documents: existing extracted_text is not re-extracted | confirmed | our code |
| 23 | documents: calamine text changes as listed | **partially-true** | dep-src |
| 24 | documents: Form XObject font regression in pdf-extract 0.12 | confirmed | dep-src |
| 25 | documents: real-world docs extract byte-identically | **partially-true (the generalisation is refuted)** | new corpus of 220 app-bundled PDFs |
| 26 | documents: zip 8 `["deflate"]` reads real OOXML/iWork the same | confirmed | new corpus of 281 real Office/iWork files |
| 27 | documents: flate2 backend switch persists nothing | confirmed | our-code grep |
| 28 | documents: none of the four crates handles secrets | confirmed | call-site grep |
| 29 | rmcp: MCP tool schema to the LLM is byte-identical | confirmed (default lifecycle) | dep-src + feature check |
| 30 | rmcp: no rmcp type persisted; ToolAnnotations serde unchanged | confirmed | grep + dep-src |
| 31 | rmcp: annotation rewrite preserves every hint | confirmed | every hunk checked against the 3.4.1 signatures |
| 32 | rmcp: cross-origin redirect leaks X-API-Key in both versions | confirmed | reqwest dep-src |
| 33 | web: tower-http 0.7.1 CORS same except Vary | confirmed | dep-src diff |
| 34 | web: dropping Vary is harmless | confirmed (reasoning; not in a webview) | our-code grep |
| 35 | web: rustyline 18 history file compatible | confirmed | cross-version experiment |
| 36 | web: rustyline 18 installs SIGINT only during readline | confirmed (plus a new consequence) | dep-src + our code |
| 37 | web: WS shutdown frame still read by the CLI | confirmed | dep-src + our code |
| 38 | web: teloxide rustls-native-roots works | confirmed + **live TLS measured** | fake-token `getMe` to api.telegram.org |
| 39 | config: shipped TOML identical under toml 1.1.6 | confirmed | own differential |
| 40 | config: the owner's real files were not checked | confirmed (true; I did not either) | – |
| 41 | config: seconds-less times; "values with seconds unchanged" | **partially-true** | measured |
| 42 | config: toml 1.x only more lenient | confirmed | 68 inputs |
| 43 | config: no preserve_order (key order unchanged) | confirmed; HEAD has none either | cargo tree at HEAD and after |
| 44 | config: YAML used only for SKILL.md | confirmed | grep |
| 45 | config: serde_yaml_ng / serde_norway identical | confirmed | 130 inputs, values *and* error text |
| 46 | config: serde-saphyr compat residue limited to the listed spellings | confirmed | 30 extra inputs |
| 47 | config: serde-saphyr defaults would change the wire | confirmed | measured |
| 48 | gui: Tauri Rust/JS pairs on the same major.minor | confirmed | read both locks |
| 49 | gui: Lightning CSS output equivalent in the webviews | unverified | – |
| 50 | gui: JS bumps touch no persisted data or wire format | confirmed | grep |
| 51 | gui: React 19.3 changes do not affect the app | unverified (code side confirmed) | grep |
| 52 | gui: marked / DOMPurify edge cases only | unverified | – |
| 53 | gui: bun.lock v2 under Bun 1.4 | unverified | – |
| 54 | ci: keyring 4 drops the explicit features; read-compat not established | **partially-true** | read the final diff |
| 55 | ci: Rust 1.94–1.98 change no platform minimums | confirmed (Linux glibc floor not measurable here) | target-spec diffs |

## 2. Claims that do not hold as stated

### 2.1 The 1.98 Debug-escape change is far narrower than reported (claim 3)
[measured] `SP/verify/dbgall` prints, for every Unicode scalar value, whether `format!("{:?}", s)` escapes it, plus `is_alphanumeric/alphabetic/lowercase/uppercase/whitespace/numeric`, `to_lowercase` and `to_uppercase`. It ran on `+1.93.0` and `+1.98.1` (`SP/logs/verify/dbgall-diff.txt`). Across 1,112,064 scalars, **the only differences are four Debug escapes: U+115F, U+1160, U+3164 and U+FFA0** (the Hangul fillers).
- Variation selectors (U+FE00–FE0F, so emoji VS16), ZWJ, U+034F, U+180B, U+E0100–E01EF and U+E0001 were **already** escaped by 1.93.0.
- `"❤️"` prints as `"❤\u{fe0f}"` on **both** toolchains (`SP/logs/verify/dbgesc-*.txt`). The toolchain report's example, "❤️ now prints as ❤\u{fe0f}", is therefore wrong.
- The rest of the claim holds. `escape_hidden`/`needs_escape` uses its own table (`crates/openalpaca_core/src/utils/text.rs:69-120`) [our-code].
- No `{:?}` of a user string reaches a persisted format. The one Debug-derived hash with a user-facing role, `list_filter_fingerprint`, formats enums only (`memory/service/mod.rs:1689-1693`).

### 2.2 There is no Unicode 17 update between 1.93 and 1.98 (claim 4)
[measured] `char::UNICODE_VERSION` is `(17, 0, 0)` on **both** 1.93.0 and 1.98.1 (`SP/verify/uver`). The per-character case and class tables are identical for every scalar (§2.1). For example, U+16EA0 (Beria Erfe, new in Unicode 17) already lowercases to U+16EBB on 1.93.0.
- The release-notes line "Update to Unicode 17" sits under 1.94.0 in `RELEASES.md`, but the shipped 1.93.0 already carries it.
- So nothing in the memory index's `to_lowercase` normalisation changes at all. The conclusion that no file is orphaned holds, and more strongly than stated.

### 2.3 Real-world PDFs are not all byte-identical (claim 25)
[measured] I built a new corpus: 396 PDFs under `/Applications`, `/Library/Documentation` and `/Library/Application Support`, 220 of them distinct. I ran them through the documents group's own harness (`SP/scratch-docs/docs-cmp-min`) with `SP/verify/pdfsweep.py` (`SP/logs/verify/pdfsweep.txt`). Result: **219 same, 1 different**, and 55 of the PDFs carry more than 2 KB of text.
- The differing file is `/Library/Documentation/License.lpdf/Contents/Resources/hi.lproj/License.pdf`.
- Uncapped, 0.8.2 returns 200,251 bytes and 0.12.1 returns 198,190 bytes.
- 0.8.2 decodes this font wrongly: `p` for every space (12,871 times), `—` for `,`, `y`/`x` for `)`/`(`, `“` for `-`, `ﬁ` for `/`, `”` for `.`.
- 0.12.1 matches Apple PDFKit (`SP/scratch-docs/pdfkit_text`). The change is a fix, but it is a change in what the model reads.
- The named en/zh/ja/ar licences were not re-run by me.

### 2.4 toml 1.x changes values that do have seconds (claim 41)
[measured] `SP/verify/tomldt` (`SP/logs/verify/tomldt.txt`, `tomlcmp2.txt`):
- `a = 1979-05-27T07:32:00.000Z` displays and serialises as `1979-05-27T07:32:00Z` under toml 0.9.12.
- Under 1.1.6 it becomes **`1979-05-27T07:32:00.0Z`**.
- So "values with seconds are unchanged" is false for an all-zero fractional part. The seconds-less cases reproduce as reported.
- This reaches `config_codec::toml_to_json` (plugin initialize payload, `GET /v1/extensions/{kind}/{id}/config`) and any toml-1.1 render.

### 2.5 The macOS keychain is reached on another path (claim 13)
[our-code] The *keyring crate* is used only at `apps/openalpacad/src/services/llm.rs:25` (use_keychain), `:68` (reverse migration) and `apps/openalpaca/src/commands/ai_config_helpers.rs:38` (use_keychain). But `crates/openalpaca_llm/src/keys/credential_discovery/mod.rs:458-475` reads the macOS keychain through `security find-generic-password -s <service> -w` (Claude Code / Codex credential discovery).
- That path does not go through `keyring`, so the keyring bump does not touch it.
- The literal claim "the OS keychain is reached only when…" is still false.

### 2.6 calamine's unescape is broader than CR/TAB (claim 23)
[dep-src] `REG/calamine-0.36.1/src/utils.rs:117-146` turns **any** `_x00HH_` into `char(0xHH)`, not only `_x000D_`/`_x0009_`. New XLSX uploads can therefore put C0 controls (NUL, ESC, …) into `file_assets.extracted_text` and into the model's input, where 0.26.1 kept the literal text.
- The whitespace-trim change is confirmed at `xlsx/mod.rs:2901-2920`.
- I did not re-run the other listed items.

### 2.7 Keyring features (claim 54)
The final `SP/wt-secrets` diff reads `keyring = { version = "4.2", default-features = false, features = ["v1"] }`. That names `v1` explicitly, the same set as the default (`REG/keyring-4.2.0/Cargo.toml` `default = ["v1"]`). It is not the bare `keyring = "4"` the claim describes.
- Read-compatibility is established from source: by the secrets group, and by my own re-read (claim 12).
- It is still not measured on a real keychain.

### 2.8 vtable miscompile exposure (claim 5, not data)
[docs] https://github.com/rust-lang/rust/issues/161441 confirms the bug: a 1.98.0 regression on aarch64-apple-darwin, fixed on nightly-2026-07-16. `RELEASES.md` 1.98.1 lists the fix. But the trigger is **native async-fn-in-trait with custom vtable erasure** (Rama's `BoxService`).
- Our 90 files use `#[async_trait]` (boxed futures), and a grep finds no dyn-erasure crates.
- So "this codebase uses that pattern" is not established. Pinning 1.98.1 remains the right call.

## 3. Confirmations by an independent method (highlights)

- **Secrets (7, 8)**: `SP/verify/aesoracle` compiles HEAD's `key_encryption/mod.rs` (== main checkout, `diff` empty) against aes-gcm 0.10.3 / base64 0.22.1, and the migrated file against 0.11.1 / 0.23.1. It then uses **ring 0.17.14 as an independent AES-256-GCM oracle** (`SP/logs/verify/aesoracle.txt`):
  - 18,030 checks, 0 failures: ring-encrypted `aes256:` values decrypt in both versions, both versions' output decrypts in ring, and the wire lengths are equal.
  - Inputs covered empty, 1 MiB, NUL, ZWJ/VS16, CJK and random text.
  - The report's fixture decrypts under ring, old and new.
  - Both `ensure_at` write 64 lowercase-hex bytes at mode 0600, and each loads the other's key.
  - An uppercase or whitespace-padded `OPENALPACA_MASTER_KEY` behaves the same in both.
  - `Generate::generate()` → getrandom `SysRng` (`REG/crypto-common-0.2.2/src/generate.rs:41-44`; aes-gcm 0.11.1 has `getrandom` among its default features).
- **SHA-256 (9, 10)**: Python `hashlib` reproduces the owner keys `32acea7e…d450` / `5c357145…0e71` / `bbc5e661…2c2f` / `9bfb77d3…07a0`. All 10 rewritten sites in `secrets.diff` use zero-padded `{b:02x}`, none unpadded `{b:x}`. `SP/verify/shafmt`: `{:x}` **and** `{:X}` → E0277 on sha2 0.11.
- **base64 (11)**: `SP/verify/b64` ran 679,320 checks over four engines, lengths 0–299 plus 511–1 MiB+1, with inserted `\n`/`\r\n`/space/`=`/URL-safe/non-ASCII/NUL mutations. Accept/reject and bytes are identical. The only Display difference is `InvalidLastSymbol` (15,457 cases).
- **Storage (15–18, 20)**:
  - `PRAGMA compile_options` lists are identical on 3.51.1 and 3.53.2 (`SP/logs/verify/sqlopts-*.txt`). `foreign_keys` defaults to 1, FTS5 unicode61 + trigram work, and the header is write/read version 2 with schema format 4 on both.
  - A 3.53.2 process exited with a 100 MB un-checkpointed WAL. **macOS's system SQLite 3.51.0** replayed it: integrity ok, 3000 rows, the trigram FTS query works.
  - 4 ALTER forms new in 3.53 (ADD CONSTRAINT CHECK, SET/DROP NOT NULL, DROP CONSTRAINT) store ordinary `CREATE TABLE` text. System 3.51.0 and Android-SDK 3.50.6 read `schema_version = 46`, pass integrity, and insert.
  - A transaction dropped without commit rolls back on rusqlite 0.38.0 and 0.40.2, with identical `execute`/`execute_batch`/`query_row` semantics. Only a UTF-8 error text changed.
  - Wider REAL→TEXT search: no f64 bound into a non-REAL column, no dynamic float `ToSql`, no `||`/CAST/printf/date function on REALs, and rusqlite's `serde_json` feature is off.
- **toml_edit (6)**: `SP/verify/tedit-0.25.{13,15}` (separate builds, since semver-compatible versions cannot coexist) produce byte-identical output over 50 inputs: 7 shipped, 42 adversarial, and my own string/float/datetime/array-of-tables file. The output covers parse→display, re-encoding **every value fresh** (the encoder the MCP manager's `toml_edit::value(...)` path uses), `fmt()`, and 6 randomised edits per file through a verbatim copy of `render_config_preserving`'s `apply_delta`.
- **toml 0.9 → 1.1 (39, 42, 43)**:
  - `SP/verify/tomldt` over 68 inputs: the shipped files are SAME, only `t04` is newly accepted, and nothing is refused.
  - `cargo tree -e features -i toml@…` at HEAD and in `wt-config`: **no preserve_order on either side**, so key order is unchanged.
  - serde_json features are identical in `wt-toolchain`/`wt-combined`/`wt-rmcp` (`alloc`, `std`, `raw_value`; no `preserve_order`, `arbitrary_precision` or `float_roundtrip`).
- **YAML (44–47)**:
  - `SP/verify/yamlng`: serde_yaml 0.9.34 = serde_yaml_ng 0.10.0 = serde_norway 0.9.42 on 130 inputs, error strings included.
  - `SP/verify/yamldiff` (30 new inputs: CRLF, BOM, tabs, block scalars, anchors, number spellings, null variants, tags, keys, flow, escapes, big ints, NBSP): every serde-saphyr difference falls in the listed categories (`017`/`010`, `1_000`, `.inf`, custom tag, big ints, `~` key), or is newly lenient (a BOM or a `%YAML 1.1` directive now parse).
  - Default options measured to turn `yes/no/on/off/y/n/N` into booleans, merge `<<` and refuse duplicate keys.
- **Documents (21, 24, 26)**:
  - lopdf 0.34's nom parser recurses array → `_direct_object` → array with no depth limit (`REG/lopdf-0.34.0/src/nom_parser.rs:257-328`). 0.42 threads `MAX_NESTING_DEPTH = 100` (`reader.rs:456`).
  - The daemon extracts in `spawn_blocking` on a runtime with no `thread_stack_size` (`background.rs:135`, `main.rs:179-182`).
  - Form XObject: `REG/pdf-extract-0.12.1/src/lib.rs:1571-1579,1712,1860-1868,2408` vs 0.8.2 `:1576`.
  - zip: **281 distinct real Office/iWork files** (Microsoft Word `.dotx` templates, `.thmx`, `.xlsx`, a `.numbers`) extract identically, old vs new, with `["deflate"]` only (`SP/logs/verify/zipsweep.txt`).
- **rmcp (29–32)**:
  - `Tool.input_schema: Arc<JsonObject>` is plain serde, and the default `serve()` lifecycle is `Initialize` offering 2025-11-25 (`REG/rmcp-3.4.1/src/model.rs:170-178`, `service/client.rs:705,721`).
  - ToolAnnotations: the fields and serde attributes are identical. Every production hunk maps by name onto `from_raw(title, read_only, destructive, idempotent, open_world)` (`tool.rs:100-114`), and `is_destructive` still defaults to `true`.
  - Redirect: `REG/reqwest-0.13.{4,5}/src/redirect.rs:239-252` strips only Authorization/Cookie/Proxy-Authorization/WWW-Authenticate, with the default `Policy::limited(10)`. Our client sets secrets via `default_headers` (`crates/openalpaca_mcp/src/transport/http.rs:96-99`).
- **Web (33–38)**:
  - tower-http: `permissive()` and every `allow_*` emitter are identical; the only change is `update_vary_header` (`REG/tower-http-0.7.1/src/cors/mod.rs:460-480`).
  - rustyline: `SP/verify/rlhist` (`SP/logs/verify/rlhist.txt`) shows identical history bytes, 0600, `#V2`, cross-loading both ways, and a v1 file loading.
  - tungstenite `Message`/`CloseFrame` are byte-identical in 0.28 and 0.30.
  - **Live TLS** [measured]: a teloxide 0.17 `rustls-native-roots` probe called `getMe` on api.telegram.org with a fake token and got `RequestError::Api` (`SP/logs/verify/tgprobe.txt`), so the TLS handshake with native roots works on this machine. A corporate CA was not tested.
- **Platform floors (55)**:
  - `rustc --print deployment-target` gives aarch64 11.0 and x86_64 10.12 on both toolchains.
  - `RUSTC_BOOTSTRAP=1 … --print target-spec-json` for the six release targets: identical for Windows. macOS drops only `llvm-floatabi`. Linux adds `supports-fentry`, plus `--fix-cortex-a53-843419` on aarch64. The glibc floor is not observable on macOS.

## 4. New data-safety risks no group named

1. **sha2 0.11 changes `{:?}` of a digest silently.** Debug of a digest goes from `[186, 120, …]` to `Array([186, 120, …])` [measured, `SP/logs/verify/shafmt-run.txt`]. `{:x}` fails to compile, but `{:?}` compiles.
   - No current site formats a digest with `{:?}` (grep of `wt-secrets`).
   - Put a note in the sha2 commit, and prefer the shared `sha256_hex` helper for any new code.
2. **toml 1.1 re-spells `…:SS.000` datetimes as `…:SS.0`** (§2.4). This affects plugin config JSON and any toml render.
3. **pdf-extract 0.12 changes the text of real non-Latin-script PDFs** (§2.3). It is a fix, but it changes what the model reads for new uploads. It also changes what retryable `Error` rows produce: a PDF that failed with "encryption scheme not supported" is retried while `attempts <= extraction_retry_count` (`background.rs:96-105`) and can flip to `Ready`.
4. **calamine 0.36 lets C0 control characters (NUL, ESC) into `extracted_text`** through `_x00HH_` unescaping (§2.6). Anything that prints extracted text to a terminal would carry ESC sequences. The approval-surface escape does not cover extracted text.
5. **rustyline 18 opens a window where Ctrl+C kills the REPL.** 18 restores the pre-readline SIGINT disposition when each readline ends (`REG/rustyline-18.0.1/src/tty/unix.rs:163-169`). Until the stream's first `tokio::signal::ctrl_c()` poll registers a handler (`apps/openalpaca/src/chat_stream/mod.rs:413`), SIGINT has the default action. So Ctrl+C while the first message's POST is being sent terminates the REPL, where 17's editor-lifetime handler swallowed it. [inferred from source; not measured]
6. **rmcp 3.4.1 (latent, not default).** If a server negotiates protocol ≥ 2026-07-28 (`STANDARD_HEADERS`), the streamable-HTTP client:
   - silently drops tools whose `x-mcp-header` annotations are invalid (`REG/rmcp-3.4.1/src/transport/streamable_http_client.rs:93-115`, warn only);
   - copies annotated tool arguments into `Mcp-Param-*` request headers, which the pre-existing redirect behaviour (claim 32) would replay cross-origin.

   The default lifecycle offers 2025-11-25, so this needs a non-compliant server or a future config change. The response cache (SEP-2549) is already covered in the MCP report.
7. **Error texts change (user-visible, low).**
   - rusqlite 0.40's UTF-8 conversion error reads `invalid utf-8 sequence … at index 0` in place of `Conversion error from type Text …`.
   - base64 0.23's `DecodeError` Debug now prints the Display text. Our code formats it with Display.

## 5. What the plan should add

1. Before the documents commit: record in the commit and in the plan that pdf-extract 0.12 changes extracted text for some real PDFs (the Hindi licence is a reproducible example). Also note that retryable `Error` rows will be re-extracted.
2. In the config commit, name the `.000` → `.0` datetime spelling beside the seconds-less change.
3. In the calamine commit, decide whether `_x0000_`/`_x001B_` should be stripped before `extracted_text` is stored.
4. In the sha2 commit, add a note (or a clippy `disallowed_methods`-style guard) against Debug-formatting digests.
5. For rustyline 18: consider installing the tokio `ctrl_c` handler once at REPL start rather than per stream, or measure Ctrl+C at a real terminal before and after.
6. The keyring bump still needs its measured round trip on a disposable keychain, as the secrets report proposes. Nothing here replaces that.

## 6. What I ran (all under `SP/verify/`, logs in `SP/logs/verify/`)

| Experiment | Command | Result |
|---|---|---|
| dbgesc, dbgall, uver | `cargo +1.93.0 / +1.98.1 run` | 4 Debug differences across all scalars; Unicode 17.0.0 on both |
| lock parse | `python3` over `git show HEAD:Cargo.lock` and `wt-toolchain/Cargo.lock` | 81 changed = dry run + takecell; no data crate moved |
| aesoracle | `cargo +1.98.1 run --release` | 18,030 checks, 0 failures; fixture decrypts under ring |
| shafmt | `cargo +1.98.1 run` / `check --features lowerhex|upperhex` | E0277 for `{:x}` and `{:X}`; `{:?}` compiles with changed output |
| b64 | `cargo +1.98.1 run --release` | 679,320 checks, 0 behaviour differences |
| sqlopts-0.38.0 / -0.40.2 | `cargo +1.98.1 run` | options identical; tx rollback on both; REAL→TEXT change reproduced |
| newsyntax, walx | 0.40.2 writer; `/usr/bin/sqlite3` 3.51.0 and Android 3.50.6 readers | all readable |
| tedit-0.25.13 / -0.25.15 | `cargo +1.98.1 run` over 50 files | outputs `cmp`-identical |
| tomldt | toml 0.9.12 vs 1.1.6 over 68 files | 3 differences: t03, t04, `.000` |
| yamldiff, yamlng | serde_yaml vs serde-saphyr (compat and default), ng, norway | as described in §3 |
| pdfsweep, zipsweep | the documents harness over new corpora | 219/220 PDFs same; 281/281 Office/iWork same |
| rlhist | rustyline 17.0.2 vs 18.0.1 | compatible |
| tgprobe | teloxide 0.17 rustls-native-roots, fake token | `Api` error, so TLS works |
| tspec | `RUSTC_BOOTSTRAP=1 rustc -Z unstable-options --print target-spec-json` | §3 |
| cargo tree (read-only) | `--locked --offline` in wt-toolchain, wt-config, wt-combined, wt-rmcp | features as stated |
