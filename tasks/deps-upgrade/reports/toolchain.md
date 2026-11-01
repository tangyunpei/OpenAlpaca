# Toolchain group: Rust 1.93.0 → 1.98.1, and the semver-compatible lock refresh

Researcher: toolchain group. Date: 2026-09-25. Worktree: `SP/wt-toolchain` (detached at `5e5ac317`).
`SP` = `/private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/4b1ee495-f087-45a9-9fa8-4325993552c8/scratchpad`.

Evidence tags: **[measured]** (I ran it; command and result given), **[our-code]** (file:line in this repo),
**[dep-source]** (a file under `~/.cargo/registry/src/...` or a repository file at a named tag),
**[docs]** (release notes, changelog, issue or PR page). Anything not established is said to be not established.

## 0. Verdict in five lines

1. Moving the pin to **1.98.1** costs nothing that CI would notice. It adds **no compiler warnings or errors**, changes **no rustfmt output**, and adds **17 clippy warnings** (33 raw). CI does not deny warnings, so they fail nothing. [measured]
2. The pin has to be **1.98.1, not 1.98.0**. 1.98.0 miscompiled vtables for boxed `dyn` async-trait objects on aarch64-apple-darwin, and this codebase uses that pattern throughout. [docs]
3. The toolchain surfaces one **new future-incompatibility report**: `proc-macro-error2 2.0.1`, reached through `teloxide 0.17.0 → aquamarine 0.6.0`. No fixed release exists upstream. It is not an error yet. [measured]
4. Raising `rust-version` to 1.98 adds **no** clippy warnings. The lock refresh on 1.98.1 moves the same 79 packages as the 1.93 dry run plus `takecell 0.1.1 → 0.1.2`. The build, clippy (identical counts) and tests stay green. [measured]
5. Recommendation: set **`rust-version` equal to the pin (1.98)** and change the pin, `rust-version` and the CI toolchain input in one commit. Put the lock refresh in a separate commit after it. The two commits are safe to land in that order.

## 1. What I ran

Every cargo command ran as `nice -n 10 env CARGO_TERM_COLOR=never CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=<dir> cargo +<toolchain> …` from inside the worktree, so the worktree's `.cargo/config.toml` applied. Target dirs: `SP/target-tc` (1.98.1) and `SP/target-tc193` (1.93.0).

Test runs also set `HOME=SP/testhome-tc`, with `CARGO_HOME` and `RUSTUP_HOME` pointed at the real ones. That kept every test away from `~/.openalpaca`. Logs are in `SP/logs/tc-*`. Warnings were counted from `--message-format=json` using `SP/tools-tc/lintcount.py` and `lintdiff.py`. A "unique" count is one per (code, file, line): `--all-targets` reports a lib's warning once for the lib and once for its test target.

| Step | Command (abbreviated) | Result |
|---|---|---|
| A | `+1.93.0 clippy --workspace --all-targets --exclude openalpaca_gui --message-format=json` | exit 0, 2m43s cold. **108 raw / 80 unique warnings, 19 lint codes, all clippy; 0 rustc warnings; 0 errors.** No future-incompat report (no `.future-incompat-report.json` in `target-tc193`) |
| B1 | `+1.98.1 build --workspace --exclude openalpaca_gui` | exit 0, 4m07s cold. **0 warnings, 0 errors.** One note: future-incompat for `proc-macro-error2 v2.0.1` |
| B2 | `+1.98.1 report future-incompatibilities --id 1` | `warning[E0365]: extern crate proc_macro is private and cannot be re-exported` at `proc-macro-error2-2.0.1/src/lib.rs:494` (issue rust-lang/rust#127909) |
| B3 | `+1.98.1 clippy … --message-format=json` | exit 0. **141 raw / 97 unique, 26 codes, all clippy; 0 rustc warnings** |
| B4 | `+1.98.1 fmt --all -- --check` and `+1.93.0 fmt --all -- --check` | Both exit 1 with **byte-identical** output (`cmp`): 1456 hunks in 290 files. The tree was already not rustfmt-clean, and CI does not run fmt. **The toolchain adds no fmt churn** (rustfmt 1.8.0 → 1.9.0) |
| B5 | `+1.98.1 test --workspace --exclude openalpaca_gui --no-fail-fast` | 24 result lines: **4165 passed, 1 failed, 13 ignored**. The failure was `managers::mcp::tests::a_stdio_server_respawns_transparently_then_reports_crashed_after_four_failures` (`"Unknown tool: 'srv__echo'"`), a pre-existing flake (§4) |
| B6 | `+1.98.1 clippy -p openalpaca_gui --all-targets` | Without a sidecar the build script fails: `resource path bin/openalpacad-aarch64-apple-darwin doesn't exist`. **I created an empty placeholder at `apps/openalpaca-gui/src-tauri/bin/openalpacad-aarch64-apple-darwin` in my worktree only** (gitignored by `src-tauri/.gitignore:10`). Then exit 0, 2 warnings, both pre-existing `let_and_return` in `openalpaca_storage`, none in the GUI crate. The same run on `+1.93.0` gave the same 2 warnings |
| C | `+1.98.1 clippy --fix --allow-dirty --workspace --all-targets --exclude openalpaca_gui` | 28 files, +145/−174. **Two machine-applied suggestions broke the build** (§3). After two hand repairs: 17 unique warnings left, full test suite 4166 passed / 0 failed / 13 ignored. Reverted with `git apply -R` |
| E0 | pin `1.98.1` + `rust-version = "1.98"`, then `+1.98.1 metadata --locked` | exit 0, `Cargo.lock` byte-identical (`cmp`). **The pin alone does not touch the lock** |
| E1 | clippy with `rust-version = "1.98"`, before the refresh | **Identical to B3** (97 unique; `lintdiff` empty). Raising the MSRV unlocks no MSRV-gated clippy lint in this code |
| E2 | `+1.98.1 update` | "Locking 80 packages to latest Rust 1.98 compatible versions". Same set as `SP/cargo-update-dryrun.txt` (79 at 1.93) **plus `takecell 0.1.1 → 0.1.2` (requires Rust 1.96)**. Lockfile stays `version = 4`. `Cargo.lock` +331/−281 |
| E3 | build, clippy, full tests, GUI clippy on E2 | build exit 0 (same future-incompat note, nothing else). Clippy **identical to B3**. Tests **4165 passed / 1 failed / 13 ignored**; the failure was `a_notification_that_arrives_around_a_disable_is_superseded` (`connect timed out after 10s`), the same MCP flake (§4). GUI clippy exit 0 with `tauri 2.11.6`, the same 2 pre-existing warnings |
| D | `+1.98.1 update --breaking --dry-run` | exit 101: "`--breaking` flag is unstable, and only available on the nightly channel" |
| D | `rustc +1.98.1 --edition 2027` / `--edition future` | "must be one of: <2015\|2018\|2021\|2024\|future>" / "latest edition supported by this Rust version is 2024". **No edition after 2024** |

## 2. What the toolchain adds (B3 vs A, unique locations; rust-version still 1.93)

No lint disappeared and no rustc lint appeared. Every new warning is from clippy:

| Lint (clippy) | +uniq | Sites | Fixable by `--fix`? |
|---|---|---|---|
| `useless_borrows_in_formatting` (new in 1.97) | 6 | `apps/openalpaca/src/commands/config_tui.rs:1113,1154,1559,1561,1563`; `crates/openalpaca_core/src/runner/lead_agent/tools.rs:621` | yes |
| `drain_collect` | 2 | `crates/openalpaca_plugins/src/manager.rs:950,959` | yes (`mem::take`) |
| `result_large_err` (now also fires on async fns, 1.98) | 2 | `apps/openalpacad/src/routes/extensions.rs:684` (`Result<(), axum Response>`); `crates/openalpaca_mcp/src/client.rs:663` (`rmcp::service::ClientInitializeError`, ≥504 bytes) | no: needs a boxing decision |
| `unnecessary_sort_by` | 2 | `crates/openalpaca_core/src/session_log/sweep.rs:330` (fixed); `session_log/record.rs:425` (suggests `Reverse`, not auto-applied) | 1 of 2 |
| `collapsible_match` | 1 | `crates/openalpaca_core/src/orchestrator/skill/output/mod.rs:45` | yes. Equivalent here, because only the `_ => {}` arm follows |
| `question_mark` | 1 | `crates/openalpaca_core/src/orchestrator/intent/mod.rs:148` | yes |
| `for_kv_map` | 1 | `apps/openalpaca/src/commands/ai_config.rs:270` | yes |
| `unneeded_wildcard_pattern` | 1 | `apps/openalpacad/src/event_bridge.rs:37` (`created_by: _, ..`) | yes, but **a judgement call**: the explicit `created_by: _` may be deliberate documentation (CLAUDE.md says `created_by` is ignored on purpose, R79) |
| `useless_format` | +1 | `apps/openalpaca/src/commands/memory/tests.rs:1080` | yes |

By crate: CLI 7, core 5, plugins 2, daemon 2, mcp 1. [measured: `SP/logs/tc-AB-lintdiff.txt`, `SP/logs/tc-B-newlints-rendered.txt`]

## 3. `clippy --fix` (step C)

- The raw output (`SP/patches/toolchain-clippyfix-raw.diff`) covers all 97 warnings, the 80 pre-existing ones included. It does not compile. [measured]
  - **E0592** `duplicate definitions with name new`, at `crates/openalpaca_llm/src/keys/secret_store/mod.rs:180`. The `new_without_default` fix put its `impl Default` between `#[cfg(test)]` and `impl MemorySecretStore`. That moved the attribute onto the new impl and left `new()` unconditional beside the `#[cfg(not(test))]` twin. Repair: put `#[cfg(test)]` on both impls (judgement).
  - **E0034** `multiple applicable items in scope`, at `crates/openalpaca_llm/src/config/settings_service.rs:193`. The `needless_borrow` fix `configured.contains(&provider_type)` → `contains(provider_type)` breaks method resolution. Repair: revert the hunk (judgement: clippy is wrong here).
- After the two repairs (`SP/patches/toolchain-clippyfix.diff`, 28 files, +145/−173): **14 of the 17 toolchain-added warnings are gone**. Three remain: `result_large_err` ×2 (structural) and `unnecessary_sort_by` at `record.rs:425`. The total drops from 97 to 17 unique. The full test suite passes: 4166/0/13. [measured]
- The output of `--fix` is not rustfmt-formatted; the let-chain indentation is off, for example. Since the tree is not fmt-clean anyway, formatting only the touched files would add churn to the diff. [measured: diff inspection]
- Worktree restored: `git apply -R`, then `git status` empty and fmt output identical to B4. [measured]
- Nothing forces these fixes (CI has no `-D warnings`: `.github/workflows/ci.yml:35`). They are an optional, separate commit, and it should cover the 17 new sites at most, not the 80 old ones. [our-code]

## 4. The one failing test is a pre-existing flake, not the bump

- B5 and E3 each failed one different test in `apps/openalpacad/src/managers/mcp/tests.rs`. Both show a stub stdio server that never answered: `connect timed out after 10s`, or `spawn_count() == 0` at `:945`. [measured]
- Reruns of the 51 MCP-manager tests (`-p openalpacad --bin openalpacad -- managers::mcp::tests::`): [measured]
  - 1.98.1 with the old lock: 5/5 pass at 14:36, 2/6 pass at 15:05.
  - 1.98.1 with the refreshed lock: 2/5 pass.
  - **1.93.0 on the unmodified tree: 3/6 pass**, with the same signatures in different tests on each run.
- So this flakiness predates the bump. It is load- and timing-sensitive; load average was 5–37 during the runs, from other sessions' builds. Neither the toolchain nor the lock refresh causes it.
- Its root cause is not established. The stub is a `/bin/sh` script written, chmod-ed and exec-ed per test (`tests.rs`, `stub_server`), and the tests use a 10 s connect timeout and a 1 s request timeout.
- About 38 orphaned `sleep 3600` children were alive at 15:0x. CLAUDE.md already records these as pre-existing. [measured: `ps`]

## 5. Rust 1.94.0 → 1.98.1: compatibility notes checked against this code

Source: `RELEASES.md` at tag `1.98.1` (`SP/research-tc/RELEASES-1.98.1.md`), the Cargo changelog (`doc.rust-lang.org/nightly/cargo/CHANGELOG.html` → `SP/research-tc/cargo-changelog.txt`) and the clippy `CHANGELOG.md` (master). [docs]

The compile-time notes are covered wholesale by B1/B3/B5/E3 compiling every target on 1.98.1 with no error or rustc warning [measured]. These include:
- closure-capture changes (1.94), std macros through the prelude against our 157 glob imports (1.94), and the stricter `dyn` lifetime casts;
- non-exhaustive match reads (1.95), `{self}` struct imports (1.96), never-type tuple coercion (1.96);
- ambiguous-import and ambiguous-glob hard errors (1.98), `assert_eq!` temporary scope (1.98), `repr(transparent)` strictness (1.98);
- linker-output warnings (1.97): none on macOS. Linux and Windows linkers were not measured.

Runtime and behaviour notes, each checked by reading our code:

| Release | Note | Our exposure | Evidence |
|---|---|---|---|
| 1.98 | **More characters escaped in `Debug` of `str`/`char`**: `Default_Ignorable_Code_Point` minus Cf/Cn, e.g. variation selectors U+FE00–FE0F (emoji VS16), Hangul fillers, U+034F, U+E0100–E01EF | **User-visible** wherever a user string reaches a person through `{:?}`: error strings, CLI output, tool results. `❤️` now prints as `❤\u{fe0f}`. Debug-derived hashes (`compose/fingerprint.rs:189`, `compose/static_prompt.rs:61,83`) change value but are in-memory only; no fingerprint column in any migration. The approval-surface escape `escape_hidden` uses its own table (`crates/openalpaca_core/src/utils/text.rs:64-120`) and is unaffected. No test asserts on it (suites pass) | [docs] PR rust-lang/rust#155527; [our-code]; [measured] tests |
| 1.98 | `derive(PartialOrd)` fast path when deriving `Ord` | All 12 `Ord` types derive both traits; no manual `PartialOrd`/`Ord` impls anywhere | [our-code] grep |
| 1.98 | `std::env::Vars{,Os}` no longer `Send`/`Sync` | No `env::vars` use | [our-code] grep |
| 1.98 | Windows TLS destructors use FLS | Windows builds only; not measured | [docs] |
| 1.97 | **v0 symbol mangling by default** | Backtrace text changes (e.g. in `daemon.log` with `RUST_BACKTRACE`); nothing in the tree parses backtraces or symbols (no `backtrace`/`Backtrace` use) | [docs], [our-code] |
| 1.97 | `pin!` deref-coercion fix | Only `tokio::pin!` is used (`runner/steering.rs:383`, `commands/confirmations.rs:387`, `commands/tail.rs:174`) | [our-code] |
| 1.97 | `std::char` constants and functions deprecated; hidden `f64` methods removed | No `std::char::…` use | [our-code] |
| 1.97 | Windows `WSAESHUTDOWN` → `BrokenPipe` | No `ErrorKind::Other` matching anywhere | [our-code] |
| 1.97 | enum layout changes | No `transmute`, no `repr(transparent)`, no raw-byte persistence of enums | [our-code] |
| 1.96 | `BTreeMap::append` may panic with an inconsistent `Ord` | Every `.append(` is on a `Vec` or `OpenOptions` | [our-code] |
| 1.95 | `Eq::assert_receiver_is_total_eq` deprecated | Not used | [our-code] |
| 1.94 | **Unicode 17** tables in std | Memory-index normalisation uses std `char::to_lowercase` (`crates/openalpaca_storage/src/memory_index/query.rs:130-131`) on indexed text and on queries alike. Index rows written by a 1.93 binary survive the upgrade: the disposable index rebuilds on `SCHEMA_VERSION` change, not on toolchain change. Only characters whose case mapping changed in Unicode 17 could miss, which is negligible. `normalize_statement` (`memory_files/note.rs:310-322`) is compared at runtime only (`memory/learning/extractor.rs:571,597,608`). `slugify` (`store/artifact.rs:144-167`) is ASCII after NFKD from the `unicode-normalization` crate, which the refresh does not move, so artifact and note file names are unaffected | [our-code], [docs] |
| 1.94 | Windows `SystemTime::checked_sub_duration` before 1601 | No `SystemTime` subtraction (`checked_sub` hits are on integers) | [our-code] |
| — | std `DefaultHasher` (SipHash) values are not stable across releases | Used only for the in-memory approval cache (`security/confirmation.rs:252-258` → `security/sandbox/mod.rs:268`). Not persisted | [our-code] |

**Future incompatibility** (new with the toolchain): `teloxide 0.17.0 → aquamarine 0.6.0 → proc-macro-error2 2.0.1`, lint E0365 / `pub_use_of_private_extern_crate` (rust-lang/rust#127909). [measured] The 1.93.0 run compiled the same crate (`tc-A-clippy193.stderr:279`) and wrote no report. crates.io API: latest `proc-macro-error2` is 2.0.1 (2024-09-06), `aquamarine` 0.6.0, `teloxide` 0.17.0, so no fix is published. It becomes a hard error only in a future Rust. Options when that happens: a `[patch]`, or a teloxide release that drops `aquamarine`, a docs-only dependency. [measured: curl crates.io API]

**Platform floors.** The six host targets the release scripts build (`scripts/release/package-macos.sh:37-40`, `package-linux.sh:30-33`, plus Windows MSVC) keep identical notes at both tags:
- `aarch64-apple-darwin` 11.0+; `x86_64-apple-darwin` 10.12+, Tier 2 with host tools at both tags;
- `x86_64-unknown-linux-gnu` kernel 3.2+ / glibc 2.17+; `aarch64-unknown-linux-gnu` kernel 4.1+ / glibc 2.17+;
- `x86_64-pc-windows-msvc` Windows 10+; `aarch64-pc-windows-msvc`.

[dep-source: `src/doc/rustc/src/platform-support.md` at tags `1.93.0` and `1.98.1`, diffed]

**Cargo, 1.94 → 1.98** [docs, cargo changelog]:
- Lockfile format stays v4 [measured: header after `update`].
- The MSRV-aware resolver has not changed; resolver 3 defaults to `incompatible-rust-versions = "fallback"`, which our `.cargo/config.toml` sets explicitly.
- `cargo update --breaking` is **still nightly-only** [measured], and the Cargo 1.100 notes say the unstable `--breaking` is being removed entirely.
- New stable features the plan could use but need not: `build.warnings` config (1.97, a replacement for `-Dwarnings`); config `include` (1.94); TOML 1.1 in manifests (1.94, raises dev MSRV); `resolver.lockfile-path` (1.97).
- Security fixes in Cargo itself: 1.94.1 (tar CVE-2026-33055/33056), 1.96 (CVE-2026-5222/5223, crate-tarball symlinks), 1.96.1 (libssh2 CVEs).

**What 1.98.1 fixed.** A vtable miscompilation introduced in 1.98.0 (rust-lang/rust#161441): "emits a zero method entry in a compiler-generated vtable… the process segfaults at address zero". It was reported on aarch64-apple-darwin with a boxed async service trait object. This codebase dispatches through boxed `dyn` async traits (`async_trait` in 8 crates) and axum/tower services, so **1.98.0 must not be used; pin 1.98.1**. The earlier point releases 1.96.1 (MIR miscompilation) and 1.97.1 (LLVM miscompilation) are listed as fixed in their own point releases. Whether both fixes are contained in 1.98.x is inferred from the release branching, not verified. [docs]

**Newly stable APIs that would remove a workaround in our code** (none required):
- `AtomicU64::update` (1.95) would replace `let _ = self.next_spill.fetch_update(Relaxed, Relaxed, |current| { …; Some(number + 1) })` at `crates/openalpaca_core/src/session_log/mod.rs:517-522`. The `let _ =` and the `Some` exist only because `fetch_update` is fallible. A cosmetic, optional change. [our-code, docs]
- Nothing else qualifies:
  - The hand-rolled `while !is_char_boundary(..)` loops (`runner/agentic_loop/tool_helpers.rs:25,29,51`, `memory/service/mod.rs:1658`, `store/artifact.rs:194`) could use `floor_char_boundary`, but that has been stable since **1.91** and is already used at `utils/text.rs:38`, so the bump does not unlock it.
  - `Vec::push_mut` does not fit `memory/learning/window.rs:332-339`, where the push is conditional.
  - E1 measured that clippy, given MSRV 1.98, suggests no `if let` guard or other newer-API rewrite.
  - No `from_utf16`, `substr_range`-style pointer arithmetic or `strip_prefix … strip_suffix` pattern exists.

**Edition:** none newer than 2024 exists (rustc 1.98.1 lists `2015|2018|2021|2024|future`, and `future` is unstable). [measured]

## 6. What the lock refresh adds (step E)

- It moves the 79 packages of the 1.93 dry run plus `takecell` (80). [measured]
- Five packages enter the graph:
  - `base64 0.23.1`, via `hyper-util 0.1.21`;
  - `darling 0.24.1` and `darling_core`/`darling_macro`, via `serde_with_macros 3.23` from tauri-utils and teloxide-core;
  - `core_detect` and `multiversion_no_op`, via `encoding_rs 0.8.42`.
- `tinyvec_macros` leaves. [measured: `cargo tree -i`, `SP/logs/tc-E-tree-added.txt`]
- Data-relevant crates confirmed **unchanged** in the lock: `libsqlite3-sys 0.36.0` (the SQLite build, hence the DB file format), `rusqlite`, `aes-gcm 0.10.3`/`aes` (secret encryption), `sha2`, `blake3`, `keyring`, `serde_json`, `serde`, `chrono`, `unicode-normalization`, `regex`/`regex-syntax` (the `\p{Cf}` table that `utils/text.rs` is held to), `tokio`, `hyper`, `axum`, `rmcp`. [measured: lock comparison]
- Moved packages that touch data or wire paths:

| Package | Move | Path | Evidence |
|---|---|---|---|
| `toml_edit` | 0.25.13 → 0.25.15 | The format-preserving config writer for `llm.toml`/`daemon.toml` (M3). The source diff is an encoder refactor (`write_str` for `write!`, merged key-path encoders), with no intended output change. Config tests pass. Byte-identical output for every input is **not established** | [dep-source] diff; [measured] tests |
| `encoding_rs` | 0.8.35 → 0.8.42 | Text decoding for uploads (calamine, pdf-extract/lopdf, multer). 0.8.40 fixed "buffer boundary handling in two-byte legacy decoders" and rewrote the ASCII acceleration. It can change `extracted_text` for **new** uploads in legacy encodings; stored rows are untouched | [dep-source] `README.md` release notes |
| `rustls` 0.23.43 → 0.23.45, `aws-lc-rs`/`aws-lc-sys` 0.44 → 0.45, `rustls-platform-verifier` 0.7.1, `hyper-rustls`, `tokio-rustls` | patch | TLS to the LLM providers, Discord and Telegram | [measured] tests; changelogs not read |
| `process-wrap` | 9.1.0 → 9.1.1 (released 2026-09-23) | rmcp's child spawner for MCP stdio. "Avoid tracing after fork", "stop capturing unused signal mask" | [dep-source] `CHANGELOG.md` |
| `uuid` | 1.26.0 → 1.26.1 | Only `timestamp.rs`/`v7.rs` changed; we use v4 | [dep-source] diff |
| `rand` | 0.10.2 → 0.10.3 | Fixes to `WeightedIndex`/`Uniform` only. Our uses are backoff jitter and discovery-token bytes (`rate_limiter/mod.rs:148`, `discovery/mod.rs:77`) | [dep-source] `CHANGELOG.md` |
| `tauri` 2.11.6, `tauri-plugin-dialog` 2.7.3, `tauri-plugin-fs` 2.5.2, `quick-xml` 0.42 (plist) | patch/minor | The GUI shell. Clippy clean; not run as an app | [measured] |
| `clap` | 4.6.7 | CLI parsing | [measured] tests (249 CLI tests pass) |

## 7. MSRV (`rust-version`) policy — recommendation: equal to the pin

Facts:
- `publish = false` (`Cargo.toml:34`).
- Every member inherits `rust-version.workspace = true` (all 11 manifests).
- `.cargo/config.toml` sets `[resolver] incompatible-rust-versions = "fallback"`.
- CI builds only the pinned toolchain (`ci.yml:22-25`).
- The README tells contributors that rustup fetches the pin (`README.md:26`).

[our-code]

What the MSRV-aware resolver does, measured on a one-dependency probe crate (`SP/msrv-probe`):

| Declared `rust-version` | Resolver behaviour | Measured result |
|---|---|---|
| **1.93** (lower than the pin) | Prefers versions whose `rust-version` ≤ 1.93, even when run by 1.98.1: "Locking … to latest Rust 1.93 compatible version". If none fits the requirement, it **falls back silently** to an incompatible one | `takecell = "0.1"` → **0.1.1** (0.1.2 held back). `takecell = "0.1.2"` → locks 0.1.2 "(requires Rust 1.96)" with no error |
| 1.93 + a dependency needing 1.96 | Nothing enforces our declared MSRV: cargo checks dependency `rust-version` only against the *running* rustc | **Builds with 1.98.1 without a word.** `cargo +1.93.0 build` refuses: "rustc 1.93.0 is not supported by … takecell@0.1.2 requires rustc 1.96" |
| **1.98** (equal to the pin) | Picks the newest versions the pinned compiler can build | `takecell` → **0.1.2**. Workspace E2 locked 80 packages "to latest Rust 1.98 compatible versions" |
| unset | Falls back to the running toolchain's version | `+1.93.0` → 0.1.1 |

Clippy's MSRV-gated lints read `rust-version`. On this code, 1.93 vs 1.98 made no difference (E1). [measured]

Dependencies whose latest release needs more than 1.93 (the `latest_msrv` column of `SP/direct-deps.tsv`):
- **Direct: `sysinfo` 0.39.6 needs 1.95.** It is used by `openalpaca` and `openalpaca_storage` (T29, `daemon_lifecycle`). Moving to it is a semver-major requirement change for the deps/storage group.
- **Transitive: `takecell` 0.1.2 needs 1.96**, via `teloxide-core 0.13.0`.
- Every other direct dependency's latest declares ≤ 1.89 (`calamine`, `keyring`, `rmcp`, `zip` 1.88; `twilight-*` 1.89; the rest lower).
- 13 declare none, which the resolver treats as compatible.

With the MSRV at 1.93:
- `cargo update` keeps holding back anything that raises its MSRV above 1.93. Today that is only takecell, but the set grows with every release.
- A manifest bump such as sysinfo 0.39 still resolves through fallback and builds on the pin. The declared 1.93 then becomes **false with no signal**; only a 1.93 CI job would catch it, and none exists.

Recommendation:
- Set **`rust-version = "1.98"` (= the pin's minor)** and move pin, `rust-version` and `ci.yml`'s `toolchain:` together from now on.
- A lower MSRV is only worth declaring with a CI job that builds on it (e.g. `cargo +<msrv> check`). Nothing needs that: no consumer builds this workspace with another compiler.
- "1.98" rather than "1.98.1" is enough, because the pin enforces the patch release that fixes the vtable miscompilation.

## 8. Risks to persisted data, secrets, wire formats and user-visible behaviour

- **Persisted data.** No toolchain-dependent value is persisted: no `DefaultHasher`, `TypeId`/`type_name`, `transmute` or layout-dependent bytes reach disk, and the Debug-based hashes are in-memory. Of the persisted derivations, only the memory index's normalised text depends on std's Unicode tables. The index is disposable, and the exposure is limited to characters Unicode 17 changed. [our-code]
- **DB format.** SQLite comes from `libsqlite3-sys 0.36.0` (bundled), unchanged by both steps. No migration. [measured]
- **Secrets.** `aes-gcm`/`aes`/`keyring` are unchanged. The encrypted-key format and the keychain path are untouched. No test touched the real keychain or `~/.openalpaca`: tests ran with a sandboxed `HOME`. [measured]
- **Config files.** `toml_edit` 0.25.15 re-implements parts of the encoder, and config tests pass. Byte-for-byte identity on every hand-edited `llm.toml` is not established. A cheap check before merge is to round-trip the owner's real `llm.toml`/`daemon.toml` through `config_io` in a temp dir and diff the bytes.
- **Wire formats.** `serde_json`, `axum`, `hyper`, `tokio-tungstenite` and `rmcp` are unchanged; `reqwest` 0.13.5 and `sse-stream` 0.2.6 are patch moves. [measured]
- **User-visible.**
  - Debug escaping of variation selectors and Hangul fillers (1.98).
  - v0-mangled names in backtraces (1.97).
  - Possibly different `extracted_text` for new uploads in legacy two-byte encodings (`encoding_rs`).
  - Nothing in the GUI crate changes (clippy identical).
- **Process.**
  - The memory evaluation (L8) is running now, and its reports record `rustc` (`scripts/eval/memory/reports/baseline-A-2026-09-24.json:12`). Landing the toolchain commit mid-L8 splits the runs across two compilers. Land it after L8, or accept a recorded change of `rustc`.
  - CI's `Swatinem/rust-cache` keys on the rustc version, so the first CI run after the bump is a cold build. [docs, not measured]

## 9. Proposed commits, in order

1. **`build: pin Rust 1.98.1; workspace rust-version 1.98`**
   - Changes: `rust-toolchain.toml` `channel = "1.98.1"`; `Cargo.toml` `rust-version = "1.98"`; `.github/workflows/ci.yml:24` `toolchain: 1.98.1`, kept equal so rustup never needs an implicit install; `CLAUDE.md:152`, `README.md:26`, `README.md:208` → 1.98.1.
   - No `Cargo.lock` change (E0).
   - Verification: `cargo +1.98.1 metadata --locked`; `build`, `test` and `clippy --workspace --exclude openalpaca_gui --all-targets` (expect 0 rustc warnings and 97 unique clippy warnings); `cargo check -p openalpaca_gui` by hand (CI never compiles it); `cargo fmt --all -- --check` output unchanged against the parent.
   - Measured: B1/B3/B5/B6/E1.
2. **`chore(deps): refresh Cargo.lock within semver (cargo update on 1.98.1)`**
   - Changes: lockfile only, 80 packages (the 1.93 dry run plus takecell 0.1.2).
   - Verification: the same build/clippy/test set, GUI clippy, the config round-trip check from §8, and one MCP stdio smoke through the daemon (process-wrap moved).
   - Measured: E2/E3 green except the pre-existing MCP flake.
   - Land it before any semver-major bump, so each major's lock diff stays small.
   - The diff of commits 1 and 2 combined (pin + `rust-version` + lock; no CI or docs edits) is `SP/patches/toolchain.diff`.
3. *(optional, may be skipped)* **`style: address clippy lints new in Rust 1.94–1.98`**
   - Scope: only the 17 sites in §2, not the 80 older ones.
   - 13 mechanical fixes. One judgement: whether to keep `event_bridge.rs:37`'s explicit `created_by: _` (then `#[allow]` it). Three structural sites (`result_large_err` ×2, `record.rs:425`) are fixed or allowed by hand.
   - Do **not** run a blanket `clippy --fix`. It breaks the build in two places (§3).
   - Verification: clippy count drops by 17; full tests.
   - Reference patch (all warnings, repaired, tests 4166/0): `SP/patches/toolchain-clippyfix.diff`.
4. *(tracking, no commit)* The `proc-macro-error2` future-incompat note: watch teloxide/aquamarine, and `[patch]` it if a future Rust turns it into an error.

## 10. Observations outside this group's scope (not acted on)

- `apps/openalpaca-gui/src-tauri/Cargo.lock` has been tracked since `2d5e847b` (487 packages). Cargo ignores it, because the GUI crate is a workspace member and uses the root lock. It is stale and no refresh touches it. [our-code, measured `git log`]
- A compiled Mach-O binary `rust_out` is tracked at the repo root (added in `96ebdc16`). [our-code]
- The workspace is not rustfmt-clean (1456 hunks / 290 files), and CI has no fmt gate. [measured]
- The MCP stdio stub tests flake around 50 % on this loaded machine on 1.93.0 as well (§4).

## Artefacts

- Patches:
  - `SP/patches/toolchain.diff`: pin + `rust-version` + lock refresh. Compiles; tests green but for the pre-existing flake.
  - `SP/patches/toolchain-clippyfix.diff`: repaired `--fix`. Compiles; tests 4166/0.
  - `SP/patches/toolchain-clippyfix-raw.diff`: raw `--fix` output. **Does not compile.**
- Logs: `SP/logs/tc-A-*`, `tc-B-*`, `tc-C-*`, `tc-E-*`, `tc-D-*`; lint tables `tc-A-lintcounts.txt`, `tc-B-lintcounts.txt`, `tc-AB-lintdiff.txt`.
- Research copies: `SP/research-tc/` (RELEASES.md at 1.98.1, cargo and clippy changelogs, platform-support.md at both tags).
- Probe: `SP/msrv-probe/` (target dir `SP/target-tc-probe`).
- Worktree state at hand-off: the `toolchain.diff` state (pin, `rust-version`, refreshed lock), plus the gitignored empty sidecar placeholder.
