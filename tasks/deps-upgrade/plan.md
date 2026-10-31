# Toolchain and dependency upgrade — research and plan

**Status: rev 2.1 — research complete, nothing applied; revised after a completeness review.** Nothing in this document has been committed, pushed or applied to the repository. The evidence it cites has been copied out of the temporary scratch directory into `tasks/deps-upgrade/` (see *Where the evidence lives now*, below, and Appendix A).

| rev | date | change |
|---|---|---|
| 1 | 2026-09-25 | First version. It synthesises the research of nine groups (toolchain, secrets, storage, documents, MCP client, web, config, GUI/JS, CI and supply chain), the data-safety skeptic's pass and the integrator's replay of the whole set as 28 ordered commits. |
| 2 | 2026-09-25 | Closes the completeness critic's 22 gaps. **Packaged-build identity:** `prepare-sidecar.ts` can bundle a stale or debug daemon; E5/E7 now build through a fixed sequence and pass a new identity gate, G-ident (measured version markers in the binaries). **Isolation of manual runs:** new §5.2 rule — store, config dir, `HOME`, CWD, daemon binary, WebKit data, and credential discovery, which with an isolated `HOME` falls back to the real macOS keychain (newly found, read in our code). **G-GUI** reworded: never create a placeholder where the real sidecar exists, and give it its own target dir (tauri-build copies the sidecar into it, measured). **DU-10/E6** list every config file. **Appendix A** preserves the E6 tools and the replay driver, removes the nine research worktrees, and records the wave in `tasks/todo.md`. **New E8:** a rehearsal on a copy of the real store. **DU-6** adds the `audio/mp3` and `heix` connector refusals. **DU-13's recommendation changes to (b)** (a v1 lock), because Dependabot's bun updater bundles Bun 1.3.14 today and rejects v2 locks (docs, fetched). §8 gains a per-PR checklist and the Tauri pairing rule, applied through Dependabot `ignore`. **New DU-21** covers the advisories gate, now with `bun audit`, and **P34 is relabelled Recommended**. **New DU-22** covers `--locked` in CI. **P21** regenerates `docs/api` (measured stale). **New commits:** P01a and P40 add and remove the temporary Windows job; P01b and P39 add and remove the conditional ubuntu-24.04 pin. **Low-severity fixes:** one freshness rule for both ecosystems; a descendant-process check in P22; P14's fixture method and effort; calamine separator collisions (DU-9); corrected test anchors; P04 added to §5.1(3); the branch tip re-checked at `908050f1` (measured); the paused LLM-table design ordered after the wave; WebView2/webkit2gtk under DU-14; `libssl-dev` in P13; `CARGO_TERM_COLOR=never`; DU-5 (d); a lopdf row in §3.1. |
| 2.1 | 2026-09-25 | Appendix A's copy step done: the plan, the reports, and one archive (`evidence.tar.gz`: `patches/`, `tools/`, `logs/`) now live in `tasks/deps-upgrade/`, untracked. The nine research worktrees are removed. No finding changed. |

**Evidence tags used throughout:**

| Tag | Meaning |
|---|---|
| **[M]** | Measured: a group ran it, and the command and log are in its report. |
| **[C]** | Read in our code (file:line). |
| **[D]** | Read in dependency source (a path under `~/.cargo/registry/src/…`, or a repository file at a named tag). |
| **[Docs]** | Inferred from a changelog, migration guide, advisory or release note. |
| **[NE]** | Not established. Nobody could settle it. |

"Skeptic" marks the independent data-safety pass's verdict. Where it corrected a group, this document states the corrected version and does not repeat the original.

Two conventions for references:
- `SP` = `/private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/4b1ee495-f087-45a9-9fa8-4325993552c8/scratchpad`.
- **Where the evidence lives now** (rev 2.1). `SP/reports/*.md` → `tasks/deps-upgrade/reports/` (this plan is `tasks/deps-upgrade/plan.md`). `SP/patches/` and `SP/logs/` → `patches/` and `logs/` inside `tasks/deps-upgrade/evidence.tar.gz`. The Appendix A tools → `tools/` inside the same archive: `tomlcmp`, `yamlcmp`, `tedit`, `tedit-0.25.13`, `tedit-0.25.15`, `lintcount` (was `SP/tools-tc`), `sqlite-probe` (was `SP/probe`, without `data*/`), `secrets-compat` (was `SP/scratch-secrets/compat`), `docs/` (`make_fixtures.py`, `cmp/`, the two fixtures) and `replay/` (was `SP/int`'s scripts). Unpack the archive **outside** the repository (`tar -xzf evidence.tar.gz -C <dir>`): `sqlite-probe` holds two copies of `openalpaca_storage`, and several tool manifests have no `[workspace]` table, so cargo run from inside the repo would find the root workspace. Everything else under `SP` (the scratch worktrees' remaining files, the target dirs, rev 2's `rev2-*` exports) was not kept.
- File:line citations are at `5e5ac317`, the research base, unless a line says otherwise. The version citations in §7 were re-read at `29b308ca`, the branch tip at rev 1. Rev 2's re-checks ran at `908050f1`.

---

## 0. Summary

**Where the tree is.**
- Branch `feat/markdown-memory` at `908050f1` when rev 2 was written, unpushed and 284 commits ahead of `origin/main` (`4256b77c`) [M: `git rev-list --count`]. The tip moves while other work lands: it was `29b308ca` at rev 1, and `dcc94410` earlier the same afternoon.
- Rust **1.93.0** is pinned in `rust-toolchain.toml:2`; `rust-version = "1.93"` in `Cargo.toml:32`; edition 2024, resolver 3, `publish = false`.
- 68 direct Rust dependencies; **26** have a newer release [M: `SP/direct-deps.tsv`]:
  - **16** need a semver-incompatible requirement change: aes-gcm, base64, calamine, infer, keyring, lru, pdf-extract, rmcp, rusqlite, rustyline, sha2, sysinfo, tokio-tungstenite, toml, tower-http, zip.
  - **10** need only a lock refresh: clap, console, rand, reqwest, rustls, tauri, tauri-plugin-dialog, thiserror, toml_edit, uuid.
  - `serde_yaml` is at its last release, which is deprecated and archived. Replacing it is a choice, not a bump.
- GUI: **16** JS packages are behind, two of them majors (Vite 7 → 8, Vitest 4 → 5). Bun is pinned nowhere: CI floats (it ran 1.4.2), and the machine that wrote `bun.lock` runs 1.3.13.
- CI: `actions/checkout@v4` runs on the Node 20 runtime GitHub removed on 2026-09-23. The Rust version is pinned twice (the toml and `ci.yml:24`); CI's copy is already overridden [M: CI log].

**Where "latest stable" is.**
- Rust **1.98.1**: RELEASES.md dates it 2026-09-03; the brief's 2026-09-01 is its commit date [Docs].
- Every crate and package version named in §1, as of 2026-09-25.
- **axum 0.9, teloxide 0.18 and twilight 0.18 do not exist**, not even as pre-releases [M: crates.io API], so those stay where they are (§3).

**The research base.**
- The groups worked on detached checkouts of `5e5ac317`. The GUI group used `caf4152e`, one commit earlier; the only GUI-side difference is `hidden-chars.json`.
- The integrator replayed the whole set on `5e5ac317` and re-tested it on `f5199123`.
- The branch has since moved 19 commits past `5e5ac317`, to `908050f1`. Those commits touch 14 files, none of them among the 37 files the patch set edits [M: `comm` of the two file lists]. They touch the Telegram and Discord connectors, `utils/text.rs`, the CLI's `chat_stream` and `confirmations.rs`, and two manuals.
- **Rev 2 re-check at `908050f1`** [M, logs `SP/logs/rev2-*`]:
  - `combined.diff` applies cleanly (`git apply --check`, 37 files);
  - `cargo +1.98.1 check --locked --workspace --all-targets --exclude openalpaca_gui` gives 0 errors and 1 warning, the known `proc-macro-error2` future-incompat note;
  - the CLI passes 260/0/1; connectors with `telegram,discord,imessage` pass 112/0; core `utils::text` passes 16/0;
  - P28's teloxide change on top passes 56/0 for the connectors, and `check -p openalpacad` gives 0 errors.
  - The **full suite was not re-run** at `908050f1`. §5.1 item 6 makes that the wave's first gate.

**The size of the job.**

| Measure | Value |
|---|---|
| Combined patch | 37 files, 11,196 lines [M] |
| Rust source | **+267 / −207 in 24 files** [M]: rmcp's migration, sha2's 10 hex rewrites, aes-gcm's one file, the serde-saphyr switch and two format-pinning tests |
| Manifests | +27 / −21 |
| CI and JS config | +142 / −31 |
| Lock files | +1424 / −6063. This includes deleting a 5146-line dead `Cargo.lock`. |

The code change is small and mechanical. The work that remains is decisions (§4) and manual checks that CI cannot run (§6).

**How much was measured, and how.**

| Kind | What |
|---|---|
| **Measured** | Every bump compiled. The whole set passed together on macOS aarch64 with Rust 1.98.1: **4169 passed / 0 failed / 13 ignored** at `5e5ac317`, and 4186/0/13 at `f5199123`. The GUI crate: clippy clean. The JS side: four CI gates, 97 test files and 1165 tests. The integrator replayed the whole set as 28 commits in order; every Rust commit compiled, and the end state was byte-identical to the combined state. Differential tests of every on-disk and wire format that moves: AES-GCM in both directions, SHA-256 hex, base64, SQLite 3.51 ↔ 3.53 files including an un-checkpointed WAL, TOML trees and bytes, YAML parse trees, PDF/XLSX/DOCX text, upload-sniffing verdicts, REPL history. Advisory scans, duplicate counts, a live TLS handshake to api.telegram.org, and CI's current behaviour from its own logs. Rev 2 adds: the set's `check` and nearest suites at `908050f1`; `docs/api` staleness; `bun audit`'s exit codes; the daemon version markers that G-ident reads; and tauri-build's copy of the sidecar into the target dir. |
| **Read in our code** | Every call site, and what of it reaches disk, the wire or a person. |
| **Read in dependency source only** | Whether keyring 4 finds what keyring 3 wrote; sysinfo's Linux process parsing; rustyline 18's signal handlers; the Tauri CLI's JS/Rust pairing rule; setup-bun's version resolution; rmcp's handshake and where its offered protocol version comes from; process-wrap 10's rework. |
| **Inferred from docs** | The Rust 1.98.0 miscompilation; React 19.3, marked and DOMPurify notes; the Bun 1.4 lockfile format; Dependabot's bundled Bun (1.3.14) and its refusal of v2 locks; Pester 6 removals; the Rust 1.99 date; tauri 2.11.6's advisory. |
| **Not established** | Any Linux or Windows compile; a real keychain; the owner's real config, skill files, partitions and database; real MCP servers and their descendant processes; the REPL at a terminal; rendering in WKWebView, WebView2 or webkit2gtk; a packaged `tauri build`; a live Telegram message; the full suite at `908050f1`. |

**What matters most.**

1. **Nothing measured loses data.** Four places could lose or orphan data if a claim were wrong: stored API keys, the names of memory partition directories, the SQLite file and the config files. Each has a gate in §5:
   - Stored secrets decrypt in both directions, checked against an independent AES implementation [M].
   - Hash spellings are identical, and the old idiom no longer compiles [M].
   - The database is portable between SQLite 3.51 and 3.53 in both directions [M].
   - TOML parsing only became more lenient [M].

   Three gates need something only the owner can do. The keychain read-back itself rests on source reading only.
   - A count of keychain references in every real `llm.toml` before the keyring commit (DU-10).
   - A no-print equality check of every real config file and skill directory before the toml_edit, toml and YAML commits (§6, E6).
   - A rehearsal on a copy of the real store before the first real start (E8). It runs the bumped `memory doctor` and SQLite 3.53.2 over the owner's existing partitions and database, which no synthetic test covers.

   The manual runs themselves could write the owner's real config or projects unless they are isolated. §5.2 gives the rule: store, config, `HOME`, working directory, daemon binary, WebKit data, and credential discovery. With an isolated `HOME`, credential discovery falls back to the real macOS keychain [C].
2. **Two advisories are reachable today. One is fixed by a one-line manifest change, the other by the lock refresh.**
   - A ~21 KB crafted PDF upload aborts the daemon: lopdf 0.34, RUSTSEC-2026-0187. Reproduced as SIGABRT in a harness [M]; the daemon crash is inferred from Rust's abort-on-overflow.
   - rustls 0.23.43 (RUSTSEC-2026-0285) is fixed only by the lock refresh.

   Also open: quick-xml 0.31 through calamine (RUSTSEC-2026-0194/0195; whether calamine calls the vulnerable functions is [NE]), tauri 2.11.5's cross-webview IPC advisory, and 10 advisories in `bun.lock`'s dev tooling. The whole set ends with **0** RustSec vulnerabilities, and `bun audit` reports 0 [M].
3. **Some user-visible behaviour changes, and several changes need an owner decision.** Upload sniffing refuses some text files it accepts today (infer). Extracted attachment text changes (pdf-extract, calamine, zip features). The YAML frontmatter parser needs a successor. The MCP protocol version offered to servers changes. There may be a window in which Ctrl+C kills the REPL. The CSS minifier changes. All of this is in §4, DU-4 to DU-14. The wave also adds CI enforcement points, the advisories gate and `--locked`, which are owner decisions too (DU-21, DU-22).
4. **When.** The patches edit files that exist only on `feat/markdown-memory`, and the L8 evaluation records `rustc` in its reports. The recommendation (DU-1) is to start the wave after L8, after slice 4's packaged verification, and after the merge. The security-first alternative is in DU-1 (C).

---

## 1. Inventory

The "commit" column names the §5 commit that carries the change.

### 1.1 Rust toolchain

| Package | From | To | Class | Effort | Risk | Evidence | Commit | Note |
|---|---|---|---|---|---|---|---|---|
| Rust (rustc, cargo, clippy, rustfmt) | 1.93.0 | **1.98.1** | toolchain | trivial | low | M | P03 | 0 new rustc warnings; +17 unique clippy warnings; rustfmt output identical; pin 1.98.1, never 1.98.0 |
| workspace `rust-version` | 1.93 | 1.98 | config | trivial | low | M | P03 | DU-3 |

### 1.2 Rust direct dependencies with a newer release (26), plus serde_yaml

| Package | From | To | Class | Effort | Risk | Evidence | Commit | Note |
|---|---|---|---|---|---|---|---|---|
| rusqlite (+ libsqlite3-sys 0.36.0→0.38.2, hashlink 0.11.1→0.12.2; bundled SQLite 3.51.1→3.53.2) | 0.38.0 | 0.40.2 | major | trivial | low | M | P06 | 0 source lines; files portable both ways |
| lru | 0.12.5 | 0.18.5 | major | trivial | low | M | P07 | clears 2 unsound advisories |
| sysinfo | 0.38.4 | 0.39.6 | major (0.x) | trivial | low | M (macOS), D (Linux) | P08 | needs Rust ≥ 1.95 |
| base64 | 0.22.1 | 0.23.1 | major (0.x) | trivial | low | M | P09 | 0 source lines; already in the lock after P04 |
| sha2 | 0.10.9 | 0.11.0 | major (0.x) | small | low | M | P10 | 10 `{:x}` sites in 7 files |
| aes-gcm | 0.10.3 | 0.11.1 | major (0.x) | small | low | M | P11 | one file plus one test line; stored secrets measured both ways |
| keyring | 3.6.3 | 4.2.0 | major | trivial (manifest only) | **medium** | D | P12 | not measured on a keychain; DU-10 |
| pdf-extract (+ lopdf 0.34.0→0.42.0) | 0.8.2 | 0.12.1 | major (0.x) | trivial | **medium** | M | P15 | fixes RUSTSEC-2026-0187; one text regression (DU-8) |
| calamine | 0.26.1 | 0.36.1 | major (0.x) | trivial | **medium** | M | P16 | fixes RUSTSEC-2026-0194/0195; cell text changes; must move with zip |
| zip | 2.4.2 | 8.6.0 | major | trivial | low | M | P16 | `default-features = false, features = ["deflate"]` (DU-7) |
| infer | 0.16.0 | 0.22.0 | major (0.x) | trivial (+ P17) | **medium** | M | P18 | refuses some text uploads (DU-6) |
| rmcp | 0.16.0 | 3.4.1 | major ×3 | small (≈290 lines, 14 files) | low–medium | M | P19–P22 | 32 compile errors before the fix, all mechanical; protocol offered changes |
| tower-http | 0.6.11 | 0.7.1 | major (0.x) | trivial | low | M | P25 | CORS responses lose `Vary` (DU-11) |
| tokio-tungstenite (+ tungstenite) | 0.28.0 | 0.30.0 | major (0.x) | trivial | low | M | P26 | DU-11 |
| rustyline | 17.0.2 | 18.0.1 | major | trivial | **medium** | M + D | P27 | terminal check; possible Ctrl+C window |
| toml | 0.9.12+spec-1.1.0 | 1.1.6+spec-1.1.0 | major | trivial | low | M | P29 | only more lenient; two datetime spellings change |
| serde_yaml | 0.9.34+deprecated | serde-saphyr 1.3.0 (compat options) | replace | small (≈70 lines) | **medium** | M | P30 | DU-4 |
| clap | 4.6.6 | 4.6.7 | lock | trivial | low | M (suite) | P04 | |
| console | 0.16.4 | 0.16.6 | lock | trivial | low | M (suite) | P04 | one version in the lock; the "appears twice" in `cargo tree` is a dedup line |
| rand | 0.10.2 | 0.10.3 | lock | trivial | low | D + M (suite) | P04 | the fixes are in APIs we do not call |
| reqwest | 0.13.4 | 0.13.5 | lock | trivial | low | M (suite) | P04 | changelog not read |
| rustls | 0.23.43 | 0.23.45 | lock | trivial | low | M (audit, suite) | P04 | **fixes RUSTSEC-2026-0285; only the lock refresh fixes it** |
| tauri | 2.11.5 | 2.11.6 | lock | trivial | low | M (GUI clippy) | P04 | GHSA-w28w-mhc8-qvjv [Docs] |
| tauri-plugin-dialog (+ tauri-plugin-fs 2.5.1→2.5.2) | 2.7.2 | 2.7.3 | lock | trivial | low | M | P04 | pairs with `@tauri-apps/plugin-dialog` 2.7.3 |
| thiserror | 2.0.20 | 2.0.21 | lock | trivial | low | M (suite) | P04 | |
| toml_edit | 0.25.13 | 0.25.15 | lock (requirement floor raised in P29) | trivial | low | M | P04 | byte-identical output on 50 inputs (skeptic) |
| uuid | 1.26.0 | 1.26.1 | lock | trivial | low | D | P04 | only v7/timestamp code changed; we use v4 |
| *70 transitive packages*, e.g. encoding_rs 0.8.35→0.8.42, process-wrap 9.1.0→9.1.1, aws-lc-rs/-sys, takecell 0.1.1→0.1.2 | | | lock | trivial | low | M | P04 | P04 moves 80 packages in total |

Feature-only changes, no version change: `teloxide` `default-features = false` + `rustls-native-roots` (P28, DU-12); `rmcp` `default-features = false` (P23, DU-5).

At their latest release already (42): anyhow, arc-swap, async-trait, axum, blake3, bytes, chrono, colored, dashmap, dialoguer, directories, file-lock, futures-util, libc, mime_guess, nix, notify, opener, regex, serde, serde_json, serde_yaml (deprecated), shell-escape, signal-hook, similar, tauri-build, teloxide, tempfile, tokio, tokio-cron-scheduler, tokio-stream, tokio-util, tracing, tracing-subscriber, tracing-test, twilight-gateway/-http/-model, unicode-normalization, url, urlencoding, which [M].

### 1.3 GUI JavaScript packages, Bun, Node

| Package | From | To | Class | Effort | Risk | Evidence | Commit | Note |
|---|---|---|---|---|---|---|---|---|
| react, react-dom | 19.2.8 | 19.3.0 | minor | trivial | low | M | P31 | main chunk +8.85 kB gzip |
| @types/react, @types/react-dom | 19.2.18, 19.2.5 | 19.3.0, 19.3.0 | minor | trivial | low | M | P31 | |
| @tanstack/react-query | 5.102.8 | 5.103.2 | minor | trivial | low | M + D | P31 | no serialization change |
| dompurify | 3.4.14 | 3.4.16 | patch | trivial | low | M + Docs | P31 | hardening only |
| marked | 18.0.11 | 18.0.14 | patch | trivial | low | M + Docs | P31 | ≈20 edge-case HTML changes, in DocumentPreview only |
| tailwind-merge | 3.6.0 | 3.7.0 | minor | trivial | low | M | P31 | CSS hash unchanged |
| @tauri-apps/cli | 2.11.4 | 2.11.5 | patch | trivial | low | M + D | P31 | updater-only change; we have no updater |
| @testing-library/dom, @testing-library/user-event | 10.4.1, 14.6.6 | 10.4.2, 14.6.7 | patch | trivial | low | M | P31 | |
| @types/node | 26.4.0 | 26.6.2 | minor | trivial | low | M | P31 | |
| jsdom | 30.0.1 | 30.1.1 | minor | trivial | low | M | P31 | test-only |
| prettier | 3.9.6 | 3.9.9 | patch | trivial | low | M | P31 | 0 files to reformat |
| `bun.lock` transitive packages (nanoid, postcss, picomatch, rollup, …) | stale | re-resolved | lock | trivial | low | M | P31 | `bun audit`: 10 advisories → 0 |
| vite | 7.3.6 | 8.3.1 | major | trivial | **medium** | M | P32 | Lightning CSS minifier; checked in Chromium only |
| vitest | 4.1.11 | 5.0.2 | major | trivial | **medium** | M | P32 | Node ≥ 22.12; 5.0.2 was 0 days old at research |
| @vitejs/plugin-react-swc → @vitejs/plugin-react | 4.3.3 | 6.1.1 | replace (optional) | trivial | low | M | P33 | bundle byte-identical; DU-14 |
| Bun | 1.3.13 local; floating in CI (1.4.2) | 1.4.2, pinned; `bun.lock` kept at v1 while Dependabot reads v1 only | toolchain | small | **medium** | M + Docs | P01 + owner | DU-13 (b) |
| Node | unpinned (CI 22.23.2, local 24.15.0) | unchanged | — | — | low | M + Docs | — | CI's Node meets every floor; not pinned in this wave |

### 1.4 CI actions and tools

| Item | From | To | Class | Effort | Risk | Evidence | Commit |
|---|---|---|---|---|---|---|---|
| actions/checkout | `@v4` (Node 20, a removed runtime) | v7.0.1 by SHA `3d3c42e5…`, `persist-credentials: false` | ci | trivial | low | M (log) + Docs | P01 |
| dtolnay/rust-toolchain | `@master`, `toolchain: 1.93.0` | removed: `rustup toolchain install` reads `rust-toolchain.toml` | remove | trivial | low | M | P01 |
| Swatinem/rust-cache | `@v2` (= v2.9.2) | v2.9.2 by SHA `6323deb1…` | ci | trivial | low | M | P01 |
| oven-sh/setup-bun | `@v2` (= v2.2.0) | v2.2.0 by SHA `0c5077e5…` | ci | trivial | low | M | P01 |
| Pester | ≥ 5.0.0 (the image's 5.9.0) | exactly 6.2.0 (fallback 5.9.1) | ci | trivial | **medium** (not run) | C + Docs | P01 |
| `libdbus-1-dev` apt step | installed | removed (`pkg-config` kept) | ci | trivial | low | M (graph) | P13 |
| EmbarkStudios/cargo-deny-action + `deny.toml` | — | v2.1.1 by SHA `3c634983…` (cargo-deny 0.20.2), advisories only | new | small | low | M | P34 (DU-21) |
| `bun audit` job in `audit.yml` | — | on PRs, pushes to main and weekly; exits 1 on advisories [M] | new | trivial | low | M (exit codes) | P34 (DU-21) |
| Temporary `windows-latest` `cargo check` job | — | added for the wave, removed before the merge | new, temporary | trivial | low (NASM question [NE]) | — | P01a, P40 (DU-18) |
| Dependabot | none; alerts disabled on this public repo | cargo `lockfile-only`, bun minor/patch, actions; monthly, 7-day cooldown; Tauri minors ignored (DU-15 p-i) | new | trivial | low | M (parses) | P35 |

### 1.5 Removals and hygiene

| Item | Today | After | Evidence | Commit |
|---|---|---|---|---|
| `apps/openalpaca-gui/src-tauri/Cargo.lock` | tracked, 5146 lines, names tauri 2.9.5, untouched since `2d5e847b` | deleted | M: cargo and the Tauri CLI never read it, proved four ways | P02 |
| `rust_out` (a 463 KB Mach-O at the repo root, from `96ebdc16`) | tracked | deleted, plus `/rust_out` in `.gitignore` | M | P37 |
| `pnpm-workspace.yaml` | tracked, pnpm leftover | deleted | M (Bun 1.3.13 ignores it) | P37 |
| release scripts: `cargo build --locked`, `bun install --frozen-lockfile` | absent | added | C | P37 |

---

## 2. Findings

### 2.1 Data safety

A claim is load-bearing here when being wrong about it could lose or orphan data or secrets, silently change an on-disk or wire format, or change what people see.

**Stored secrets: AES-GCM (`llm.toml` `secret_encrypted`).**
- The construction [C: `crates/openalpaca_llm/src/keys/key_encryption/mod.rs:73-85, :94-218`]:
  - The key is the raw 32 bytes of `state/.master_key`: 64 lowercase hex characters, written 0600. `OPENALPACA_MASTER_KEY` overrides it. There is no key derivation.
  - AES-256-GCM with a random 12-byte nonce prefixed, the 16-byte tag appended, and no AAD.
  - The stored form is `aes256:` + base64 STANDARD.
- **Secrets written by today's build decrypt after the bump, and secrets written after it decrypt with today's build, so a rollback is safe too.** The evidence:
  - The secrets group built HEAD's file verbatim against aes-gcm 0.10.3 / base64 0.22.1, next to the migrated file against 0.11.1 / 0.23.1 [M]:
    - 10,000 plaintexts under 20 master keys cross-decrypt in both directions;
    - 149 failure inputs give identical outcomes and messages;
    - 3,000 random tuples give byte-identical ciphertext‖tag.
  - Skeptic: **confirmed with an independent oracle.** ring 0.17.14's AES-256-GCM, 18,030 checks, 0 failures [M].
- The `.master_key` format and semantics are unchanged. Keys minted by either version load in the other [M]. The migrated generator uses getrandom's `SysRng`, which is still the OS CSPRNG [D: `crypto-common-0.2.2/src/generate.rs:41-44`].
- Gate: P05's fixture test, which passes before the bump (14/14) and after it [M].

**Stored secrets: the OS keychain (keyring 3 → 4).**
- **Not measured on a real keychain**; the rules forbade it. From source (secrets group, and the skeptic's independent re-read of four crates), keyring 4 with the `v1` feature addresses the same credential keyring 3 wrote for `Entry::new("OpenAlpaca", "llm/<provider>/<uuid>")` [D]:
  - **macOS:** the same login keychain, service and account.
  - **Windows:** the same target `<user>.<service>`, Enterprise persistence, UTF-16LE.
  - **Linux:** v4 searches on `{service, username}`, a subset of the attributes v3 wrote, so v3's items match. This relies on the Secret Service's subset matching, which is our reading of the spec.
- Exposure is opt-in. The keyring crate is reached only with `[security] use_keychain = true` or by the one-time reverse migration of leftover `secret_ref` keys [C: `apps/openalpacad/src/services/llm.rs:13-83`, `apps/openalpaca/src/commands/ai_config_helpers.rs:26-42`].
  - Skeptic correction: the macOS keychain is also read **outside** the keyring crate, by credential discovery through `security find-generic-password` [C: `crates/openalpaca_llm/src/keys/credential_discovery/mod.rs:458-475`]. That path does not change with this bump.
- Worst case if the source reading is wrong: a `use_keychain` user's keys read as missing until rollback. Nothing deletes them. The settings service deletes a keychain entry only on an explicit replace or remove [C: `config/settings_service.rs:374, :403`], and the reverse migration never deletes one [C: `config/llm_config/migration.rs:135-190`].
- Windows' `windows-native-keyring-store` was never compiled; no Windows Rust build exists anywhere [NE].
- Gate: DU-10.

**Hashes that name files (sha2 0.11).**
- SHA-256 hex names:
  - the memory partition directories: `memory/owners/<first 32 hex>/`;
  - the control snapshots: `state/memory-control/<partition-key>.json`;
  - note revisions and `.history/`;
  - `file_assets.sha256` (upload dedup);
  - persona ETags.

  [C: `crates/openalpaca_storage/src/memory_files/mod.rs:139-175`, `note.rs:275`, `content_io.rs:15`]. A spelling change would orphan every partition.
- **Identical hex** on 5,021 inputs and every idiom [M]. The known answers for `owner_key` pass: `u_example_alice` = `32acea7eb739b08afd959db91e46d450`, plus bob, system and alice_reborn [M]. Skeptic: **confirmed** by a Python `hashlib` oracle and a review of all 10 rewrites (all use zero-padded `{b:02x}`).
- **The old idiom cannot drift silently.** `format!("{:x}", digest)` (and `{:X}`) is a compile error on sha2 0.11 [M].
- Skeptic, new risk: `format!("{:?}", digest)` **does** compile, and its output changes from `[186, 120, …]` to `Array([186, 120, …])` [M]. No current site does this (grep). P10's commit body and a CLAUDE.md gotcha (P38) say so.
- blake3, used for prompt fingerprints, does not move.

**The SQLite file (rusqlite 0.40.2, bundled SQLite 3.51.1 → 3.53.2).**
- No migration, no schema change; `schema_version` stays 45.
- **Files are portable both ways** [M: the storage group's probe with the real `Database::open` and `MemoryIndex::open`]:
  - a database at schema 45 written by 3.53.2 opens under 3.51.1, and the reverse;
  - an un-checkpointed WAL written by the other version replays;
  - `integrity_check` and FTS5 `integrity-check` pass, and searches return the same ids.

  Skeptic: **confirmed with a different reader.** macOS's system SQLite 3.51.0 replayed a 100 MB WAL written by 3.53.2 [M].
- **SQLite 3.53 renders REAL → TEXT with 17 significant digits** (`CAST(0.1+0.2 AS TEXT)`: `0.3` → `0.30000000000000004`) [M]. No SQL of ours converts a REAL to text:
  - every REAL column is bound and read as `f64`, and rusqlite refuses to read a REAL as `String` [C, M];
  - skeptic's wider search found no `||`, CAST, printf or date functions on REALs, and rusqlite's `serde_json` feature is off.

  So no stored value or wire format changes. A future query that converts would print 17 digits.
- The bundled compile flags are identical, so FTS5 (unicode61, trigram), foreign keys, DBSTAT and STAT4 all stay [D; skeptic: M via `PRAGMA compile_options`].
- The schema-42 refusal test passes on 3.53.2 [M].
- An older build still reads `schema_version` after 3.53-only `ALTER TABLE` forms [M, two older readers], so a newer schema is still refused cleanly.
- Transactions still roll back on drop [M on both versions].
- The upgrade **fixes** three bugs present in our bundled 3.51.1 [Docs]:
  - the WAL-reset corruption race, which applies to us because the CLI opens `state/openalpaca.db` from a second process [C: `apps/openalpaca/src/commands/config.rs:79`, `commands/memory.rs:1459`];
  - a deadlock in the broken-POSIX-lock detector;
  - wrong results from the EXISTS-to-JOIN optimisation. Whether any of our `EXISTS` queries was actually mis-planned is [NE].

**Config and frontmatter parsing (toml 1.1, toml_edit 0.25.15, the YAML successor).**
- Every shipped TOML file parses to the same tree and re-serialises byte-identically under toml 1.1.6 [M, 7 files]. The same holds for the real loaders: `load_daemon_config`, `render_config`, the M3 `llm.toml` edit, `McpConfig` plus its fingerprints, `AgentConfigFile`, and the plugin manifest [M].
- Parsing only becomes more lenient: dotted keys that extend an implicit table now parse. Nothing that parsed before is refused [M on 42 inputs; skeptic on 68].
- Two spellings change. Both reach `config_codec::toml_to_json`, i.e. the plugin `initialize` payload and `GET /v1/extensions/{kind}/{id}/config` [C: `config_codec.rs:23`]:
  - TOML 1.1 times without seconds keep their spelling (`07:32`, where 0.9 wrote `07:32:00`) [M];
  - skeptic correction: an **all-zero fraction** changes too. `1979-05-27T07:32:00.000Z` renders as `…:00.0Z`, where 0.9 wrote `…:00Z` [M]. So "values with seconds are unchanged" is not true.
- No crate enables `preserve_order`, so written key order is unchanged [M at HEAD and after].
- toml_edit 0.25.15 (arrives with P04): byte-identical to 0.25.13 on 50 inputs, including randomised edits through a verbatim copy of `apply_delta` [skeptic, M].
- **Not checked: the owner's real TOML files and user-written SKILL.md files** [NE, by rule]. There is more than one real copy. A daemon started from the checkout finds its config by walking up from the executable, so it reads the checkout's own ignored `config/llm.toml` (exists, 944 bytes [M: `ls`; `git status --ignored`]; resolution order [C: `apps/openalpacad/src/bootstrap/config.rs:14-61`]). §6 E6 lists every file. Why a parse failure matters (config report §2.1 reader table [C]):
  - an `llm.toml` the new parser refused would be **replaced by a full render** on the next settings write, losing comments and unknown keys [C: `llm_config/mod.rs:102-106`];
  - a refused `daemon.toml` silently falls back to defaults, including the cost caps [C: `daemon_config/mod.rs:148-155`];
  - a refused `plugins/.config/<plugin>.toml` reads as an **empty map** with a WARN, so the plugin runs with no config [C: `permission_gate.rs:395`];
  - a refused `plugins/.permissions.toml` is an error for the plugin approvals [C: `permission_gate.rs:532`];
  - a refused `plugin.toml` leaves that plugin unloaded [C: `manifest.rs:170`];
  - a refused `config/tools/*.toml` or legacy `config/agents/<id>.toml` is a load error [C: `tools/config/mod.rs:65`; `services/agents.rs:98`].

  Gate: E6 (§6), before P04 (toml_edit 0.25.15), P29 and P30.
- YAML is parsed only for SKILL.md frontmatter [C: `middleware/skill/mod.rs:54, :63`]. Memory notes, persona documents and agent templates use hand-written parsers [C: `memory_files/note.rs:6`, `middleware/persona_frontmatter.rs:1-8`, `agent/template/mod.rs:216`]; skeptic confirmed. So **no persisted memory data is parsed by the YAML crate.** What does travel: a skill's `scripts[].parameters` becomes the tool's JSON schema sent to the model [C: `types.rs:237, :247-251`], so the parser's reading of typeless scalars is a wire format. DU-4.

**Extracted attachment text and MIME classification.**
- **Existing rows are never re-extracted.** The worker picks only `Uploaded` rows and retryable `Error` rows [C: `apps/openalpacad/src/background.rs:88-105`]; skeptic confirmed.
  - Skeptic, new risk: an AES-encrypted PDF that failed as "encryption scheme not supported" and is still inside `extraction_retry_count` **will** be retried and can flip to `Ready`.
- **pdf-extract 0.12 / lopdf 0.42:**
  - It fixes the process abort [M in a harness; skeptic: D, `lopdf-0.34.0/src/nom_parser.rs:257-328` recurses with no depth bound, while 0.42 has `MAX_NESTING_DEPTH = 100`]. The daemon extracts in `spawn_blocking` with tokio's default 2 MiB stack [C: `main.rs:179-182`].
  - It reads AES-128/256 PDFs [M].
  - It moves pdf-extract's `println!` noise out of `daemon.log`. One PDF printed 13,698 lines before, 0 after [M].
  - It **introduces** one regression: text inside a Form XObject that reuses a page font name is decoded with the wrong font [M on fixtures; D].
  - Skeptic correction: real PDFs are not all byte-identical. Of 220 distinct app-bundled PDFs, 1 differs, the Hindi licence, where 0.12 fixes about 14,000 wrong glyph substitutions and matches Apple PDFKit [M]. The groups' 754 system PDFs were identical [M]. So what the model reads changes for some new uploads, in the correct direction.
- **calamine 0.36** changes XLSX cell text [M]:
  - it trims untagged leading and trailing whitespace;
  - it keeps rich runs that follow a plain `<t>`;
  - CRLF becomes LF;
  - it reads inline CDATA;
  - it no longer drops a whole sheet because of one odd cell.

  Skeptic correction: it unescapes **every** `_x00HH_`, not only `_x000D_`/`_x0009_` [D: `calamine-0.36.1/src/utils.rs:117-146`]. So NUL, ESC and the other C0 controls can enter `extracted_text`. Plain-text uploads can already do this: they are stored with `read_to_string` as-is [C: `extraction.rs:52-58`]. DU-9.

  **Separator collision.** The extractor joins a row's cells with a tab and ends each row with a newline [C: `extraction.rs:101-104`].
  - An escaped `_x0009_` now becomes a real tab inside a cell [M: documents report §4, `a_x000D_b_x0009_c` → `a\rb\tc`]. The model then reads one cell as two, so the table's structure changes. This is **new** with the bump.
  - A literal newline inside a cell (and `_x000A_`, which now unescapes) reads as an extra row. For literal newlines this is **pre-existing**: 0.26.1 already passed `line1\r\nline2` through as-is [M: same table].
  - DU-9.
- **zip 8 with `["deflate"]`** reads every spec-conforming OOXML and iWork file identically [M on fixtures; skeptic: 281 real Office and iWork files, identical]. bzip2- or LZMA-compressed entries (off-spec) become unreadable. DU-7.
- **infer 0.22** turns some accepted uploads into refusals [M: documents report §6 verdict table]:
  - `%PDF` anywhere in the first KiB of a declared `text/plain` or `text/html` file, or a `text/csv` that starts `AC1015,`, now gets `415 MIME_MISMATCH` on upload and is BLOCKED on connectors;
  - **two non-text cases, connector path only**: `audio/mp3` carrying MPEG-2 frames (`FF F3`), and `image/heic` with major brand `heix`, go from OK to **BLOCKED**. infer 0.22 now detects them as `audio/mpeg` and `image/heif`, and neither pair is on the sanitizer's alias list [C: `sanitizer/mod.rs:268-282` knows only m4a, wav and flac aliases]. On the upload route both were already refused; only the code changes, from `MIME_UNDETECTABLE` to `MIME_MISMATCH`.
  - Connector attachments (iMessage photos, Telegram audio) go through the sanitizer [C: `store_attachment`]. How often a `heix` HEIC or an `audio/mp3` declaration occurs is [NE].
  - Pre-existing, unchanged: an ordinary iPhone HEIC (brand `heic`) declared `image/heic` is already refused by both versions [M].
  - DU-6.
- **encoding_rs 0.8.40** (inside P04) fixed boundary handling in two-byte legacy decoders. It can change the text of new uploads in legacy encodings [D: release notes].

**MCP protocol behaviour (rmcp 0.16 → 3.4.1).**
- No rmcp type is persisted, and `ToolAnnotations`' serde shape is unchanged [C, D; skeptic confirmed].
- The tool input schema handed to the LLM is **byte-identical** [M; skeptic: D].
- Every annotation hint survives the rewrite, mapped by field name [M: tests; skeptic checked every production hunk against 3.4.1's `from_raw` signature]. So no tool changes permission tier or confirmation behaviour.
- The wire to MCP servers changes:
  - the protocol version offered moves from 2025-03-26 to 2025-11-25 [M];
  - an `MCP-Protocol-Version` header is sent on HTTP [M];
  - `ping` carries `_meta.progressToken` [M].
- The client accepts whatever version the server answers [M]. A server that **rejects** 2025-11-25 outright, instead of negotiating down, would fail its handshake [NE: no real third-party server was tested].
- Improvements:
  - a tool that returns `{"content": []}` works; on 0.16 it made the reconnect ladder loop, 32 respawns in 4 s [M];
  - an expired HTTP session re-initialises transparently [D].
- Not fixed, and pre-existing in both versions: a cross-origin 307 replays `X-API-Key` and every `extra_headers` value to the new origin [M on both versions; skeptic: D]. rmcp's own CVE fix covers only rmcp's default client, and we pass our own. See §3.
- Latent, not the default: if a server negotiates protocol ≥ 2026-07-28, the HTTP client silently drops tools whose `x-mcp-header` annotations are invalid, and copies annotated arguments into `Mcp-Param-*` headers [skeptic: D].

**Other formats checked.**
- **REPL history (`state/repl_history`):** byte-identical across rustyline 17 and 18, mode 0600, `#V2` header, loads both ways [skeptic: M].
- **GUI `localStorage`** (`oa-project`, `oa-pane-widths`, `oa-pins`): no bumped library touches it [C].
- **CORS:** every `Access-Control-*` header is byte-identical; only `Vary` disappears [M; skeptic: D]. This is harmless because `Access-Control-Allow-Origin` is the constant `*` and the daemon sets no `Cache-Control` [C]. It was not observed in a webview [NE].
- **Memory index (`state/cache/memory-index-v1.sqlite`):** disposable. It needs no `SCHEMA_VERSION` bump, and it is portable both ways [M].
- **Toolchain-dependent values:** none is persisted. `DefaultHasher` feeds only the in-memory approval cache, and the Debug-derived fingerprints are in-memory [C; skeptic confirmed with a wider grep].

### 2.2 Rust toolchain 1.93.0 → 1.98.1 and the MSRV

**What the new toolchain changes in this tree.**
- The unmodified tree on 1.98.1: `build` exits 0 with **0 warnings, 0 errors** [M].
- Clippy goes from 108 raw / 80 unique warnings (19 lints) to **141 raw / 97 unique** (26 lints). That is +17 unique, all clippy, none removed [M]:

  | Lint | Sites |
  |---|---|
  | `useless_borrows_in_formatting` | 6 |
  | `drain_collect` | 2 |
  | `result_large_err` | 2 |
  | `unnecessary_sort_by` | 2 |
  | `collapsible_match` | 1 |
  | `question_mark` | 1 |
  | `for_kv_map` | 1 |
  | `unneeded_wildcard_pattern` | 1 |
  | `useless_format` | 1 |

- CI neither denies warnings nor runs rustfmt [C: `.github/workflows/ci.yml:30-35`], so none of this can turn CI red.
- rustfmt output is **byte-identical** between the two toolchains: 1456 hunks in 290 files, all of which predate the bump [M].
- Tests on 1.98.1: 4165 passed / 1 failed / 13 ignored [M]. The failure is a load-sensitive MCP stdio-stub flake in `apps/openalpacad/src/managers/mcp/tests.rs`. It also fails 3 of 6 runs on 1.93.0 on the unmodified tree [M]. Its root cause is [NE].
- One new future-incompatibility report: `proc-macro-error2 2.0.1` (E0365), reached through `teloxide 0.17.0 → aquamarine 0.6.0`. No fixed release exists [M]. See §3.
- A blanket `cargo clippy --fix` **breaks the build** in two places [M]:
  - E0592 at `crates/openalpaca_llm/src/keys/secret_store/mod.rs:180`;
  - E0034 at `crates/openalpaca_llm/src/config/settings_service.rs:193`.

  After two hand repairs it clears 14 of the 17 new warnings, and the suite passes 4166/0/13 [M]. See DU-17.
- `cargo update --breaking` is still nightly-only, and no edition after 2024 exists [M].

**Why the pin must be 1.98.1, not 1.98.0.** 1.98.0 has a vtable miscompilation that segfaults boxed `dyn` async service objects on aarch64-apple-darwin [Docs: rust-lang/rust#161441]. Skeptic correction: the trigger is native async-fn-in-trait with custom vtable erasure. Our code uses `#[async_trait]` (boxed futures) in 90 files and no erasure crates, so **that our code hits the bug is not established**. Pinning the exact 1.98.1 stands either way. Three consecutive point releases (1.96.1, 1.97.1, 1.98.1) fixed miscompilations [Docs].

**Behaviour notes, with skeptic corrections.**
- **Debug escaping.** Skeptic correction [M over all 1,112,064 scalars, both toolchains]: `format!("{:?}", s)` changes for **four code points only**, the Hangul fillers U+115F, U+1160, U+3164 and U+FFA0. Variation selectors, VS16 (so `"❤️"`), ZWJ, U+034F and U+E0100–E01EF were already escaped by 1.93.0. The approval-surface escape (`escape_hidden`) uses its own table and is unaffected [C: `crates/openalpaca_core/src/utils/text.rs:69-120` at `5e5ac317`; the tip has since edited that file, and at `908050f1` the table is `is_hidden` at `:71`, `char::is_control` plus a fixed list, with `escape_hidden` at `:213`].
- **Unicode tables.** Skeptic correction [M]: `char::UNICODE_VERSION` is 17.0.0 on **both** 1.93.0 and 1.98.1, and every scalar's case and class results are identical. So the memory index's `to_lowercase` normalisation does not change at all. The toolchain report's "Unicode 17 arrives in 1.94" was read from RELEASES.md; the shipped 1.93.0 already carries it.
- **Backtraces.** v0 symbol mangling is the default from 1.97, which changes backtrace text. Nothing parses backtraces [C].
- **Platform floors.** Identical for the six release targets [D: `platform-support.md` at both tags; skeptic: M via `--print deployment-target` and target specs]. The glibc floor cannot be observed on macOS.

**MSRV.**
- `rust-version` only steers the MSRV-aware resolver and clippy's MSRV-gated lints. Nothing enforces it against a newer compiler.
- With 1.93 declared, a dependency needing 1.96 still locks (with a printed note) and builds on 1.98.1 with no error. Only a 1.93 build refuses it [M: `SP/msrv-probe`].
- After P08 (sysinfo 0.39.6 needs 1.95), a declared 1.93 would be false with no signal.
- Raising it to 1.98 unlocks no MSRV-gated lint here [M].

See DU-3.

### 2.3 The semver-compatible lock refresh (P04)

- `cargo +1.98.1 update` locked **80 packages**: the 1.93 dry run's 79 plus `takecell 0.1.1 → 0.1.2`, which needs Rust 1.96. The lockfile stays v4; the diff is +331/−281 [M].
- Build, clippy (unchanged, 97 unique) and tests (4165/1/13, the same flake) all pass [M]. GUI clippy is clean with tauri 2.11.6 [M].
- Skeptic **confirmed** the moved set and that the data crates did not move: libsqlite3-sys, rusqlite, aes-gcm/aes, sha2, blake3, keyring, serde, serde_json and chrono, with no checksum change for any unchanged version [M]. regex and unicode-normalization are unchanged too [M].
- It carries:
  - rustls 0.23.45 (RUSTSEC-2026-0285);
  - tauri 2.11.6 (GHSA-w28w-mhc8-qvjv, cross-webview IPC reads; small exposure because the app has one window);
  - tauri-plugin-dialog 2.7.3 and tauri-plugin-fs 2.5.2;
  - toml_edit 0.25.15;
  - encoding_rs 0.8.42, which can change legacy-encoding upload text (§2.1);
  - process-wrap 9.1.1 (rmcp's stdio spawner; fork-safety fixes; released 2026-09-23; P22 later moves it to 10.0.1);
  - reqwest 0.13.5, clap 4.6.7, console 0.16.6, rand 0.10.3, thiserror 2.0.21, uuid 1.26.1 and aws-lc 0.45.
- It adds base64 0.23.1 (via hyper-util), darling 0.24.1 (via serde_with_macros), core_detect and multiversion_no_op, and drops tinyvec_macros [M].
- **At branch time, `cargo update` will move more than these 80.** P04's gate compares the new move list with the researched one (§5).

### 2.4 Storage: rusqlite, lru, sysinfo

- **rusqlite 0.40.2.**
  - 0 compile errors; builds on 1.93.0 too [M].
  - Tests: storage 678 passed / 3 ignored (the benches), core `memory` 335, connectors+imessage 70 [M].
  - No regression in the 10K memory-index bench, beyond machine noise, in alternating runs [M].
  - The API changes reach no call of ours. The `Utf8Error` variant changes shape, so log wording changes, e.g. the iMessage reader's skip warning [D, C].
  - Everything about the file format is in §2.1.
- **lru 0.18.5.**
  - 0 compile errors [M]. `iter()` is still most-recently-used first, which the compose cache's miss attribution relies on [M; D].
  - Clears RUSTSEC-2026-0253/-0002 (unsound; not practically reachable [C]).
  - Drops hashbrown 0.15, foldhash 0.1 and allocator-api2 [M].
- **sysinfo 0.39.6.**
  - 0 compile errors, but **needs Rust 1.95**: cargo refuses 1.93.0 [M].
  - `ProcessStatus` variants are identical [D].
  - On macOS, the real-zombie test passes: an exited, unreaped child is not live [M].
  - Linux name, exe and status parsing is unchanged [D]. The measurement is CI's ubuntu run of the same test.
  - Adds `objc2-open-directory` (macOS, from the unused `user` default feature).
  - It links into the daemon, the CLI **and the Tauri GUI**, which CI never compiles.

### 2.5 Secrets and hashing: aes-gcm, sha2, base64, keyring

- **aes-gcm 0.11.1.**
  - 1 compile error before the fix (E0432, `aead::OsRng` gone), plus deprecations. The change is 24 lines in `key_encryption/{mod,tests}.rs` [M].
  - The fix: `Generate::generate()` for key and nonce, `TryFrom`/`From` for `Key`/`Nonce`, and `&nonce` [D].
  - Data safety in §2.1.
- **sha2 0.11.0.**
  - 4 E0277 diagnostics before the fix. 10 sites in 7 files were rewritten to the per-byte `{b:02x}` idiom that `memory_files::sha256_hex` already used [M].
  - The six duplicated hex helpers were deliberately **not** consolidated; that would be a refactor, not something the bump forces.
- **base64 0.23.1.**
  - 0 source lines. Encodings and accept/reject decisions are identical [M: 200,000 random decodes per engine; skeptic: 679,320 checks].
  - Only the text of the `InvalidLastSymbol` message changes. It is visible only in the error for a corrupted `llm.toml` value and in the `update_persona` tool's error [M].
  - The default `simd-unsafe` feature compiles engines our `GeneralPurpose` calls never use [D]. Trimming it is not forced (§3).
- **keyring 4.2.0.**
  - The manifest line only: `{ version = "4.2", default-features = false, features = ["v1"] }`, because v3's feature names no longer exist [M]. Skeptic correction to the CI group: the final diff names `v1` explicitly. It is not a bare `keyring = "4"`.
  - On Linux the backend moves from libdbus to pure-Rust zbus, with a DH-encrypted session [D]. The non-GUI Linux graph then has **no `libdbus-sys`** [M: `cargo tree --target x86_64-unknown-linux-gnu`], so CI's `libdbus-1-dev` becomes unnecessary (P13). The GUI still links dbus through tao.
  - About 40 new crates, Linux only [M].
  - The store opens once per process, on first use. On a headless Linux machine the failure is logged earlier and in different words; the user-visible outcome is unchanged [D, C].
- The four bumps compile on 1.93.0 for llm, storage and core [M]. The daemon and CLI were not checked on 1.93 by this group.

### 2.6 Documents: pdf-extract, calamine, zip, infer

All four compile with **zero source changes** on 1.98.1 and 1.93.0 [M]. Tests: openalpacad 47 (extraction, routes::files, background), core 76, connectors 13 [M].

- The behaviour is covered in §2.1.
- Every "Not measured" below is [NE]:
  - XLS (BIFF) extraction: no generator was available;
  - how often real uploads hit the Form-XObject case;
  - why zip 2.5.0–2.6.1 were yanked (we are on 2.4.2, which is not yanked);
  - whether a declared `image/vnd.dwg`, newly accepted by infer 0.22, is refused by providers as an image part.
- calamine 0.36 requires zip ^8.6, so the two move together [D].
- Choosing `["deflate"]` drops 23 lock entries, including the bzip2, lzma, xz2 and zstd C libraries [M].
- zip's `deflate` switches flate2 to the zlib-rs backend for **every** flate2 user in the daemon (zip and lopdf, decompression only). Nothing compresses or persists compressed bytes [M; skeptic: C].
- New advisory warning: `ttf-parser` 0.25.1 is unmaintained (RUSTSEC-2026-0192), via lopdf 0.42. It is transitive and informational [M].
- Pre-existing, unchanged:
  - an asset whose extraction crashed the daemon stays `processing` forever [C];
  - an iPhone HEIC declared `image/heic` is refused as `MIME_MISMATCH` [M];
  - the Japanese licence PDF extracts almost no text in either version [M].

### 2.7 MCP client: rmcp 0.16.0 → 3.4.1

- 32 compile-error diagnostics before the fixes: 12 in `openalpaca_mcp`, 20 in `openalpaca_core`, plus 2 same-pattern sites in the daemon fixed beforehand [M]. Code +112/−178 = 290 lines in 14 files [M].
- The fixes are mechanical:
  - `#[non_exhaustive]` model types are built through constructors (1.0): `Implementation::new`, `CallToolRequestParams::new(..).with_arguments(..)`, and 18 `ToolAnnotations` literals rewritten **by field name**;
  - `ContentBlock` replaces `Content`/`RawContent` (2.0);
  - `server_info` becomes `Option` (3.0).
- Four small judgement calls: an unset `OnceCell` when `server_info` is absent; an `_ => "unknown"` arm for the non-exhaustive `ContentBlock`; no compatibility alias for the old re-exports; updated source citations.
- Staging through 0.17 → 1.8 → 2.2 → 3.4.1 works. Each stage compiles on mcp, core and the daemon, and passes 34 mcp tests; the 3.x code does not compile against 2.x [M].
- Tests on 3.4.1: mcp 34 passed / 2 ignored (npx e2e); daemon `managers::mcp` 51; core lib 2023; `routes::tools` 8 [M].
- MSRV 1.88; builds on 1.93.0 [M].
- The handshake is still the legacy `initialize` sequence; no `server/discover` probe is sent [M, D].
- With `default-features = false` it still compiles clean and passes 34 tests, and drops schemars 1.x, rmcp-macros, darling 0.24 and others [M on the rmcp worktree only]. DU-5.
- Behaviour is in §2.1.

### 2.8 Web and CLI: tower-http, tokio-tungstenite, rustyline, the teloxide TLS stack

- All three bumps need **zero source changes** and compile on 1.98.1 and 1.93.0 [M]. rustyline 18's real MSRV is ≥ 1.89 (`std::fs::File::lock`), although it declares none [D].
- **tower-http 0.7.1.**
  - Only `Vary` disappears from CORS responses [M, probe].
  - reqwest 0.12 and 0.13 still require tower-http ^0.6.8, so both versions link into the daemon [M].
  - The daemon has no CORS test at all [M: the filter matched 0 tests]. P24 adds one.
- **tokio-tungstenite 0.30.**
  - The WebSocket shutdown contract still reads correctly: a 0.30 client against axum's 0.29 server receives the `daemon_shutting_down` frame, then `Close(1001, "daemon_shutting_down")` [M: 71 daemon tests pass, both shutdown tests among them].
  - The two tungstenite versions never link into one binary: the CLI has 0.30, the daemon 0.29 [M]. Pinning 0.29 instead would remove 8 packages [M]. DU-11.
- **rustyline 18.**
  - The CLI suite passes 249/0/1, all 8 completer tests included [M]. The history format is unchanged (§2.1).
  - SIGINT and SIGWINCH handlers are now installed only while `readline` runs; 17 kept them for the Editor's lifetime [D; skeptic confirmed].
  - Skeptic, new risk [D, inferred, not measured]: between the end of `readline` and the stream's first `tokio::signal::ctrl_c()` poll [C: `apps/openalpaca/src/chat_stream/mod.rs:413`], SIGINT has its default action. **Ctrl+C while the first message's POST is in flight would kill the REPL**, where 17's handler swallowed it. P27's terminal checklist tests exactly this.
  - rustyline now honours `NO_COLOR`; our helper draws no colour.
- **teloxide TLS.**
  - The only path to native-tls/OpenSSL in the tree is teloxide's default features → teloxide-core → reqwest 0.12. On Linux, `openssl-sys` is **not vendored** [M].
  - `default-features = false, features = ["macros", "ctrlc_handler", "rustls-native-roots"]` removes 8 crates, compiles, and passes the connector tests (39) [M].
  - Skeptic: **a live TLS handshake measured.** `getMe` to api.telegram.org with a fake token returned an API error, i.e. the handshake succeeded [M]. A corporate CA was not tested.
  - DU-12.
- Already at latest: axum 0.8.9, hyper 1.11.1, h2, http, tokio 1.53.1, teloxide 0.17.0, twilight 0.17.1 [M]. Contrary to the research brief, hyper, h2, native-tls, dashmap, dialoguer and notify each have **one** version in the lock [M].

### 2.9 Configuration formats: toml, toml_edit, serde_yaml

- **toml 1.1.6.**
  - 0 compile errors and 0 source edits [M].
  - Tests: llm 245, plugins 117, core 2024, CLI config 32, daemon config paths 74 [M].
  - It removes the duplicated `toml_datetime` and `winnow` from the daemon and CLI [M].
  - Lock trap, which the order in §5 avoids: if toml lands **before** the lock refresh, a plain `cargo update -p toml@1.1.4` moves tauri-utils back to toml 0.9.12. Doing `cargo update -p toml@1.1.4+spec-1.1.0 --precise 1.1.6+spec-1.1.0` first avoids it [M]. After P04, tauri-utils is already on 1.1.6 and no `--precise` step is needed [M, integrator].
- **serde_yaml: three honest options** (DU-4).

  | Option | What it is | Measured |
  |---|---|---|
  | A. Keep serde_yaml 0.9.34 | Archived, no RustSec advisory | — |
  | B. serde_norway 0.9.42 or serde_yaml_ng 0.10.0 | The same archived code under a new name. ng still ships the unmaintained `unsafe-libyaml`; neither fork has released in 21+ months [D]. | Identical to serde_yaml on 158 files [M]; skeptic: identical on 130, error text included |
  | C. serde-saphyr 1.3.0 with compat options | Maintained, pure safe Rust, +6 small crates. 1.0.0 shipped 2026-07-31 and releases come about every two weeks [M, crates.io]. | Identical on all 17 shipped files and 58 inline test fixtures [M]; skeptic: 30 more inputs, all within the listed residue |

  - Without the compat options, serde-saphyr's **defaults would change the model-facing wire**: `y`/`n`/`yes`/`no`/`on`/`off` (even a bare `N`) become booleans, `<<` merges, and duplicate keys are refused [M]. The patch's pin test fails without the options [M] and holds under serde_yaml 0.9.34 [M].
  - The residue with the options:
    - typeless `010` becomes 10.0 and `1_000` becomes 1000;
    - `.inf`/`.nan` become strings;
    - some inputs serde_yaml refused are now accepted;
    - a `~` map key and nesting deeper than 64 are refused;
    - error wording changes.
- **console / dialoguer:** one version each in the lock [M]. console moves in P04; dialoguer is at its latest.

### 2.10 GUI: JS packages, Bun, the Tauri JS/Rust pairing

- **The whole JS set upgrades with zero source, config or test edits,** Vite 8 and Vitest 5 included [M]. All four gates pass at every stage: 97 files, 1165 tests, 0 files for Prettier.
  - The baseline suite once hit a load-induced timeout in `ChatView.test.tsx` at load average 66.8. That predates any bump [M].
- **Vite 8.3.1.**
  - Rolldown and Oxc replace esbuild and Rollup, and Lightning CSS becomes the minifier.
  - JS total goes from 726,970 to 723,749 bytes; 19 chunks become 18; `index.html` gains 3 `modulepreload` links [M].
  - CSS goes from 48,039 to 49,457 bytes. `::backdrop` is split into its own rule, `-webkit-` prefixes are added, and the `rounded-full` sentinel becomes `2147483647px` [M]. The groups read every change as equivalent, and Chromium renders identically [M]. **It was not checked in WKWebView, WebView2 or webkit2gtk**, the webviews the app ships in [NE]. Skeptic: unverified. The bundle targets are `"all"` [C: `tauri.conf.json` `bundle.targets`], and the owner's release scripts build for Windows and Linux too. E5 covers WKWebView only; DU-14 asks the owner to accept the other two or check them by hand (E5b).
  - `@vitejs/plugin-react-swc` prints a "switch to @vitejs/plugin-react" line under Rolldown. The swap is bundle-identical [M] (DU-14).
- **Vitest 5.0.2.** None of its breaking items applies: 51 `vi.mock` calls all at top level, and both `.resolves`/`.rejects` awaited [C]. The Node floor rises to ≥ 22.12; CI's image default is 22.23.2 [Docs + M].
- **React 19.3.** The app uses no transitions, hydration, Activity, form actions or ViewTransition [C]. Tests pass [M]. Skeptic: the code side is confirmed; that 19.3 changes nothing else we use is [Docs] only.
- **`bun.lock`.**
  - The committed lock carries **10 advisories (7 high, 3 moderate)**, all in vite/vitest dev tooling (nanoid, postcss, picomatch, rollup). None ships in `dist/`.
  - Re-resolving the lock clears all 10, even on Vite 7. Bumping pins with an incremental install does not [M].
- **Bun.**
  - CI already runs Bun 1.4.2 and accepts our v1 lock with `--frozen-lockfile` [M: CI log].
  - setup-bun reads the root `package.json`'s `packageManager` when no input is given [D]. Bun 1.3.13 ignores the field [M].
  - Bun 1.4 writes **lockfile v2** for a fresh lock, which Bun 1.3.x cannot read [Docs: oven-sh/bun#28792, fetched in rev 2: "`bun.lock` default `lockfileVersion` is now `2`", and "Existing v0/v1 lockfiles continue to load"; no option to keep writing v1 is mentioned]. Whether a v1 lock stays v1 when Bun 1.4 re-saves it is [Docs, second-hand: oven-sh/bun#31602 via dependabot-core#15848; not measured]. The researched locks are v1, made by Bun 1.3.13. DU-13.
  - **Dependabot's bun updater cannot read v2** [Docs, fetched 2026-09-25]:
    - dependabot-core#15848: with Bun 1.3.5 bundled, the updater silently rewrote v2 locks as v1, re-resolving everything.
    - The fix, PR #15896 (merged 2026-08-14), makes it **reject** unsupported lockfile versions. It also bumped the bundled Bun to 1.3.14, which still reads v1 only.
    - `bun/Dockerfile` on dependabot-core `main` pins `ARG BUN_VERSION=1.3.14` today.
    - So a v2 `bun.lock` would make every Dependabot bun run fail, until Dependabot ships Bun ≥ 1.4.
- **Tauri's version guard is a no-op here.** tauri-cli asks `npm list`, which exits 1 with `(empty)` under Bun's linker, so no pair is ever compared [M, D: `tauri-cli-2.11.5/src/helpers/npm.rs:302-318`, `info/plugins.rs:34-41, 141-167`]. The major.minor pairing must be kept by hand.
  - After the plan: tauri 2.11.6 ↔ `@tauri-apps/api` 2.11.1, and tauri-plugin-dialog 2.7.3 ↔ `@tauri-apps/plugin-dialog` 2.7.3 [M; skeptic confirmed].
  - Today the Rust dialog crate is 2.7.2 against JS 2.7.3, a patch drift, which the rule allows.
- wry 0.55 and tao 0.35 cannot move: `tauri-runtime-wry` pins them [D].

### 2.11 CI and supply chain

- **CI's Rust pin is a dead copy.** The CI log says 1.93.0 is "overridden by rust-toolchain.toml" [M]. `rustup toolchain install` with no argument installs the toml's channel, profile and components [M: rustup 1.29.1 help; Docs]. Deleting the action leaves one pin and drops a third-party action that tracks a moving branch.
- **zizmor 1.30.1:**
  - today's `ci.yml`: 13 findings (6 high `unpinned-uses`, 4 medium `excessive-permissions`, 3 medium `artipacked`) [M];
  - the proposed workflows: 0 [M].
- **Advisories at HEAD** [M: cargo-audit 0.22.2, DB `e2111519`]:
  - 5 vulnerabilities:
    - lopdf RUSTSEC-2026-0187 (reachable);
    - quick-xml -0194 and -0195 (likely reachable);
    - rmcp -0189 (**not** reachable: server transport, not compiled [D]);
    - rustls -0285 (reachable);
  - 7 unmaintained crates and 3 unsound findings; 0 yanked.
- **The proposed cargo-deny gate** (`unmaintained`/`unsound = "workspace"`, `yanked = "deny"`, empty ignore list) fails HEAD with 7 errors and passes the combined lock [M]. So it must land after rmcp, pdf-extract, calamine, lru and the lock refresh.
- **Dependabot alerts are disabled** on this public repository [M: API 403]. Only the owner can turn them on.
- **CI does not fail on warnings.** Main CI on 1.93.0 (Linux) was green with 2 build warnings (unused imports at `apps/openalpacad/src/notification/imessage.rs:4`, a Linux-only cfg) and 83 clippy warnings [M]. That contradicts CLAUDE.md:76's historical "the workspace build is warning-free" on Linux.
- **Runner images.** `ubuntu-latest` moves to Ubuntu 26.04 between 2026-10-19 and 2026-11-19 [M: CI notice; Docs]. A wave PR that runs in that window could fail on the image rather than the bump (§6).

### 2.12 The combined state (integration)

- **All slices combine with no semantic conflict** [M]:
  - `check` shows 0 errors; the full suite passes **4169/0/13**;
  - clippy: **96 unique** (140 raw, 25 lints). That is one fewer than HEAD on 1.98.1: aes-gcm's migration removes a `clone_on_copy`. No dependency bump adds a lint;
  - rustfmt is neutral against HEAD (1456 hunks) after 8 layout-only fixes to lines the patches add;
  - GUI crate clippy: 0 warnings in the crate;
  - the JS gates are green;
  - cargo-deny: `advisories ok`; cargo-audit: 0 vulnerabilities and 9 allowed transitive warnings;
  - `cargo update --dry-run` locks 0 packages, i.e. every direct dependency is at its latest.
- **The order was proven** by restoring HEAD and replaying 28 commits (`SP/patches/combined-commits/01…28`):
  - every Rust commit passes the workspace check, and the targeted tests pass where a commit touches a data format;
  - the end state, `Cargo.lock` included, is **byte-identical** to the combined state [M].
- **Cargo refuses the combined state on 1.93.0**, because every member now requires 1.98 [M].
- **Duplicate crate names** (`cargo tree -d -e normal`) [M]:

  | Scope | HEAD | Combined |
  |---|---|---|
  | Workspace, host | 33 names / 71 versions | 38 / 80 |
  | Workspace, all targets | 65 / 151 | 71 / 160 |
  | `openalpacad` | 13 | 19 |
  | `openalpaca` (CLI) | 9 | 2 |
  | `openalpaca_gui` | 10 | 15 |

  The growth is the RustCrypto 0.10/0.11 generation split, which lopdf 0.42, axum's tungstenite 0.29 and tauri-codegen keep on 0.10, plus two avoidable picks (DU-11). Duplicates cost compile time, not correctness.

### 2.13 Where the reports disagree, and which evidence wins

| # | Disagreement | Winner | Why |
|---|---|---|---|
| 1 | Toolchain edits `ci.yml:24` to 1.98.1; CI deletes that step | CI group | The toml already overrides CI's pin [M: log line 301]. One pin is better than two. |
| 2 | Toolchain: 1.98 Debug escapes VS16 and emoji ("❤️ now prints as ❤\u{fe0f}") | Skeptic | Measured over every scalar on both toolchains: only 4 Hangul fillers change. |
| 3 | Toolchain: Unicode 17 arrives in 1.94 | Skeptic | `UNICODE_VERSION` is 17.0.0 on both toolchains, and the tables are identical [M]. |
| 4 | Toolchain: "this codebase uses the miscompiled pattern" | Skeptic | Not established; the trigger is async-fn-in-trait erasure, which we do not use. The pin stays 1.98.1. |
| 5 | Secrets: the keychain is reached only via `use_keychain` or the reverse migration | Skeptic | Credential discovery also reads it via `security` [C]. The bump does not touch that path. |
| 6 | Documents: real-world documents extract byte-identically | Skeptic | A new corpus found 1 of 220 PDFs differs (the new output is the correct one) [M]. Both measured; the generalisation fails. |
| 7 | Documents: calamine unescapes `_x000D_` and `_x0009_` | Skeptic | Any `_x00HH_` [D]. |
| 8 | Documents: "no quick-xml duplicate after the bump" | Integrator | In the combined order the lock refresh moves plist to quick-xml 0.42, so 0.41 and 0.42 coexist [M]. Not a security issue (0.41 carries the fixes). P16's gate says "no quick-xml ≤ 0.40". |
| 9 | Config: "values with seconds are unchanged" | Skeptic | `…:00.000Z` becomes `…:00.0Z` [M]. |
| 10 | CI: keyring 4 is taken as bare `keyring = "4"` | Secrets diff | The final line names `v1` explicitly [C: diff]; the CI group read an earlier worktree snapshot. |
| 11 | CI: daemon duplicates 13 → 20 | Integrator (19) | Measured on the real combined lock, not on an all-latest resolution. |
| 12 | rmcp group expects a new darling 0.24 duplicate; web expects 6 new RustCrypto crates; secrets expects base64 0.23 to add a package | Integrator | Order-dependent. After P04 and P10, rmcp removes darling 0.23, tungstenite 0.30 adds only `sha1 0.11`, and base64 0.23 adds nothing [M]. |
| 13 | Integrator O12: one docs commit at the end | Project convention | CLAUDE.md: docs that cite versions change in the same commit as the versions. The integrator's reason (independently cut patches collide on shared doc lines) disappears when the commits are authored in sequence, so each commit in §5 carries its own citation edits. |
| 14 | Brief: 1.98.1 released 2026-09-01 | RELEASES.md: 2026-09-03 | 09-01 is the rustc commit date [M, Docs]. |
| 15 | Briefs: tauri-plugin-dialog is 2.7.3 on both sides; there are two calamine and two lopdf; hyper, h2 and native-tls are duplicated | The locks | Dialog is 2.7.2 on the Rust side; there is one calamine and one lopdf; each of those network crates has a single version [M]. |
| 16 | CI report: Bun 1.4 changes lockfile content only for GitHub/tarball dependencies (sha512 integrity), and we have none | GUI report and oven-sh/bun#28792 | Both are true about different things. v2's new *content* is the off-registry integrity hashes. But a lock created from scratch by Bun 1.4 is `lockfileVersion: 2` whatever it contains, and Bun ≤ 1.3 cannot read that [Docs, fetched]. So P31's regeneration under Bun 1.4.2 would produce a lock that older Buns and Dependabot's bun updater cannot read. That is why DU-13 changed in rev 2. |
| 17 | Rev 1 counted "a v1 lock merges green" as the Dependabot risk | dependabot-core PR #15896 | Since 2026-08-14, Dependabot rejects a lock version it cannot read, rather than downgrading it silently [Docs, fetched]. The risk under a v2 lock is therefore that JS updates stop, with an error in the Dependabot view, not a silent downgrade. |

---

## 3. Holds, deferrals and follow-ups

### 3.1 Cannot move: waiting on upstream

| Item | Why it holds | Trigger to revisit |
|---|---|---|
| axum 0.8.9 | 0.9 is unreleased. Its `main` changes `axum::serve` (hyper's header-read timeout; output type), which would touch `main.rs` and `graceful_shutdown_tests.rs` [Docs] | axum 0.9.0 on crates.io |
| teloxide 0.17.0 / teloxide-core 0.13.0 | 0.18 is unreleased. Its master moves to reqwest 0.13, which would drop the reqwest 0.12 and tower-http 0.6 duplicates [Docs] | teloxide 0.18.0; also re-check whether it drops `aquamarine` (see the next row) |
| `proc-macro-error2` 2.0.1 future-incompat (E0365), via teloxide → aquamarine | No fixed release [M] | A Rust release that makes E0365 an error (then `[patch]` it), or a teloxide release without aquamarine |
| twilight 0.17.1 | 0.18 unreleased [M] | twilight 0.18.0 |
| wry 0.55 / tao 0.35 | Pinned by `tauri-runtime-wry` 2.11.x [D] | A tauri release that moves them |
| infer 0.19 (GUI only), toml 0.8 / 0.9, toml_edit 0.19 / 0.20, generic-array 0.14.7, matchit 0.8.4 | Held by tauri-utils, tauri-build, the gtk-rs build scripts and axum [M] | Their upstreams |
| lopdf 0.42.0 (latest 0.45.0, 2026-09-08) | pdf-extract 0.12.1, the latest release, requires `lopdf ^0.42` [M: crates.io API; D: documents report]. lopdf is the parser that sees untrusted PDFs, so later fixes matter. | A pdf-extract release on lopdf ≥ 0.43. Re-run P14/P15's PDF cases and the Form-XObject test. |
| rustls 0.24 | Only a dev pre-release exists [M] | 0.24.0 stable |
| glib 0.18 unsound (RUSTSEC-2024-0429, Linux GUI only) | Needs tauri off gtk-rs 0.18 [M] | A tauri major |

### 3.2 Not forced by the bump: left out of this wave unless a DU says otherwise

- Feature trims:
  - base64 `default-features = false, features = ["std"]` (drops the unused unsafe SIMD engines; not measured);
  - rmcp `default-features = false` (measured on its own worktree; P23, DU-5);
  - sysinfo with only `system` (drops `objc2-open-directory`; not measured).
- Consolidating the six duplicated SHA-256 hex helpers into one (a refactor).
- `AtomicU64::update` in `session_log/mod.rs:517-522` (cosmetic; stable since 1.95).
- Pinning Node with `actions/setup-node` (CI's 22.23.2 meets every floor).
- Rust 1.99.0, due about 2026-10-01 [inferred]. Trigger: the branch is cut after it ships, and DU-2 says take it.

### 3.3 Found in passing: pre-existing problems, separate work (each needs a test first)

- **MCP HTTP redirect leak (security).** A cross-origin 307 replays `X-API-Key` and `extra_headers` to the new origin, in both rmcp versions [M]. The fix is one line, `.redirect(reqwest::redirect::Policy::none())` in `crates/openalpaca_mcp/src/transport/http.rs`. **Recommended soon, as its own `fix:` commit, independent of this wave.**
- **Unbounded reconnect ladder.** `install_service` resets the attempt counter on every successful handshake [C: `crates/openalpaca_mcp/src/client.rs:325`], so a deterministic retriable error loops for ever.
- **`client_info` never sent.** Servers see `rmcp`, not the configured `openalpaca-mcp` (`NotifyingHandler` does not override `get_info`) [M]. Changing it changes what servers see; that is an owner call.
- **`openalpaca config set|unset daemon.*` drops every comment** and sorts keys [C: `daemon_config_cli/mod.rs:408, :438`].
- **The MCP stdio-stub test flake** (about 50 % on this loaded machine, 1.93.0 included [M]); root cause [NE].
- **Linux build warnings:** two unused imports at `apps/openalpacad/src/notification/imessage.rs:4` [M: CI log].
- **README's Linux prerequisites** (`README.md:29`) do not name OpenSSL headers, although the daemon's Linux build needs them until P28 (`openssl-sys` is not vendored) [M: graph]. Whether a clean Ubuntu without them fails is [NE]. Rev 2 fixes the text in P13, which edits that line anyway.
- **Two rustls crypto backends** (ring for Discord, aws-lc-rs for reqwest 0.13) in one binary [M].
- **`prepare-sidecar.ts` decides by mtime alone** [C: `apps/openalpaca-gui/scripts/prepare-sidecar.ts:45-56`]:
  - it rebuilds only when `./target/<profile>/openalpacad` is newer than the copy in `src-tauri/bin/`, and it never looks at the sources;
  - it always copies from the hard-coded `./target`, even when `CARGO_TARGET_DIR` is set (`:42-43`);
  - `tauri dev` and `tauri build` write the same destination path (`:38-39`), so a debug copy made later can be bundled into a release.

  This is pre-existing, and E5/E7 work around it (§6). A fix, for example comparing content or building unconditionally, is separate work.
- **Credential discovery reads the real macOS keychain from an isolated daemon** [C]:
  - With `HOME` pointed elsewhere, Claude Code discovery misses `$HOME/.claude/.credentials.json` and falls back to `security find-generic-password -s "Claude Code-credentials"` (`credential_discovery/mod.rs:374-420`; `dirs_path()` reads `HOME`, `:521-523`).
  - It is on by default whenever `[credential_discovery] claude_code`/`codex` are unset (`apps/openalpacad/src/services/llm.rs:178`).
  - The shipped `llm.toml` template sets neither [M: grep].

  This is pre-existing. The §5.2 isolation rule switches it off for the wave's manual runs.

### 3.4 Ordering with other planned work

- **The paused "LLM providers as a table" design** (`tasks/llm-simplify/design.md`, rev 1, paused 2026-09-23) overlaps three commits here.
  - Its §2 plans a `protocol` field in `llm.toml`, and "key storage per id (keychain entry naming, encrypted secrets)" [C: `design.md:53`].
  - That overlaps P11 (aes-gcm), P12 (keyring 4) and P29 (toml).
  - **Order: land this wave first.** The design then cites keyring 4's `v1` API and the aes-gcm 0.11 code as they stand after the wave.
  - Whenever that design changes keychain entry naming, re-run the DU-10 keychain read-back (count, then a disposable-keychain round trip if any `use_keychain` user exists).

---

## 4. Owner decisions

Each decision is recorded, once taken, as an *Owner decisions taken* row in `tasks/api-fix-plan.md` §0 with the commit that applies it, like MD-n and T-n.

| DU | Question | Recommendation |
|---|---|---|
| DU-1 | Where and when the wave happens | After L8, slice 4's packaged verification and the merge; from `main` |
| DU-2 | Which Rust | 1.98.1 |
| DU-3 | MSRV policy | `rust-version` = the pin's minor, moved together |
| DU-4 | serde_yaml successor | C: serde-saphyr 1.3 with compat options, `LastWins` |
| DU-5 | rmcp scope | In this wave, four commits; default features unchanged; offer the new protocol version, (d) held in reserve |
| DU-6 | infer's new refusals (`text/*`, and on connectors `audio/mp3` and `heix` HEIC) | (b) keep today's `text/*` verdict with a narrow rule (P17); for the two media cases, add `audio/mp3` ↔ `audio/mpeg` only |
| DU-7 | zip features | `["deflate"]` |
| DU-8 | pdf-extract Form-XObject regression | Accept; ignored test; file upstream |
| DU-9 | calamine C0 controls and separator collisions in extracted text | Accept C0; for the tab/newline collision, (b') replace TAB/LF/CR inside a cell with a space |
| DU-10 | keyring 4 gate | Count keychain references in every real `llm.toml` first; round trip if any |
| DU-11 | tower-http 0.7 / tokio-tungstenite 0.30 vs dedup picks | Latest (0.7.1, 0.30) |
| DU-12 | teloxide over rustls | Adopt with OS roots if a live bot round trip can be run; else defer |
| DU-13 | Bun version and lockfile format | **(b), changed in rev 2**: pin 1.4.2, keep the lock at v1 while Dependabot's bun updater reads v1 only |
| DU-14 | Fresh JS majors; plugin-react swap; other webviews | Take them (≥ 7 days old); take the swap; accept WebView2/webkit2gtk unless a machine is at hand (E5b) |
| DU-15 | Maintenance bot | Dependabot, SHA pins, alerts on; Tauri minors held back by `ignore` |
| DU-16 | Pester version | Exactly 6.2.0, fallback 5.9.1 |
| DU-17 | Clippy cleanup and a warnings gate | Skip in this wave |
| DU-18 | Windows coverage | A temporary Windows `cargo check` job on the wave's PR (P01a, removed by P40) |
| DU-19 | Repository hygiene | Take P37 |
| DU-20 | Migration SQL policy for older SQLite | No rule needed |
| DU-21 | Advisories gate (RustSec and `bun audit`) | Adopt on PRs and weekly; the owner answers a red weekly run |
| DU-22 | `--locked` in CI | Adopt |

**DU-1 — Where and when does the wave happen?**
- Facts:
  - The patch set edits files that exist only on `feat/markdown-memory`, e.g. the schema-42 test at `database/tests.rs:1097`. `origin/main` is 284 commits behind [M].
  - The set applies to `f5199123` and passes there (4186/0/13 [M]). At `908050f1` it applies and checks, and the suites nearest the tip's changes pass (§0) [M]. The full suite was not re-run there.
  - L8 runs from a worktree of this branch, and its reports record `rustc` [C: `scripts/eval/memory/reports/baseline-A-2026-09-24.json:12`].
  - Slice 4's packaged, network-denied daemon-and-Tauri verification is not done [C: CLAUDE.md].
- Options:
  - **(A)** After L8's runs finish, slice 4's packaged verification passes and `feat/markdown-memory` merges: cut `chore/toolchain-and-deps` from `main` and author P01–P40 there. Each verification then measures one change. Cost: the PDF abort and the rustls advisory stay open until then (they are older than this branch), and the patches may need re-cutting (the replay driver does that; Appendix A).
  - **(B)** Now, on a branch stacked on the branch tip. Fastest, and measured to apply and pass at `f5199123`. Cost: slice 4's packaged verification would then cover the memory build and the wave together, so a failure could be either. If L8 rebuilds from the stacked code, its runs split across rustc 1.93.0 and 1.98.1.
  - **(C)** Split. A security slice goes onto `feat/markdown-memory` on the current toolchain before slice 4's packaged verification: P14, P15, P16, plus `cargo update -p rustls --precise 0.23.45`. Everything else follows (A). pdf-extract, calamine and zip compile on 1.93.0 with no source change [M]; the rustls-only update on 1.93.0 is **not** measured.
- **Recommendation: (A).** Choose (C) instead if a packaged build will reach anyone who can upload an untrusted PDF or spreadsheet before the merge; a linked connector user counts.

**DU-2 — Which Rust?**
- (A) 1.98.1: every number here was measured on it.
- (B) The newest stable when the branch is cut. 1.99.0 is due about 2026-10-01 [inferred]; taking it means re-running P03's gate (clippy, fmt and test comparisons, and the future-incompat list), and the numbers in this plan become stale.
- **Recommendation: (A).** If the branch is cut after 1.99.x ships, (B) is acceptable with P03's gate re-run. Prefer an x.y.1 where one exists: three consecutive point releases fixed miscompilations [Docs].

**DU-3 — MSRV policy.**
- (A) `rust-version` equals the pin's minor ("1.98" with "1.98.1"), and the two move together.
- (B) Keep "1.93". After P08 it is false, and silently so [M, §2.2]; it is honest only with a CI job that builds on 1.93.
- (C) Something in between, with the same caveat as (B).
- **Recommendation: (A).** `publish = false`; no consumer builds with another compiler. Under (A) the resolver picks what the pinned compiler builds (takecell 0.1.2 [M]).

**DU-4 — serde_yaml successor.**
- The options are in §2.9: (A) keep serde_yaml, (B) serde_norway or serde_yaml_ng, (C) serde-saphyr 1.3 with compat options.
- What changes under each:
  - (A) Nothing. P30 is dropped. The `+deprecated` label stays, on archived code.
  - (B) A rename only (`SP/patches/config-alt-serde_norway.diff`). Behaviour identical; still dormant code.
  - (C) P30 as written. SKILL.md files parse identically for every real file measured. Exotic typeless spellings change in a skill's `parameters` schema, and one new refusal class appears (a `~` map key, nesting deeper than 64).
- Sub-question for (C): keep `DuplicateKeyPolicy::LastWins` (serde_yaml accepted duplicate map keys, so skills keep loading), or `Error` (stricter; such a skill stops loading).
- **Recommendation: (C) with `LastWins`**, as its own commit (P30) so it can be dropped alone. (A) if the owner wants zero behaviour change in this wave.

**DU-5 — rmcp in this wave, and how.**
- (A) In this wave as four bisectable commits (P19–P22). Each stage compiles and passes [M].
- (B) One squashed commit: the same ≈290 lines, one lock change.
- (C) A later wave of its own. rmcp stays on 0.16: the unreachable RUSTSEC-2026-0189 then needs a reasoned `ignore` in `deny.toml` (P34), and the empty-content respawn loop stays.
- Sub-questions:
  - `default-features = false` (P23): measured clean on the rmcp worktree only. **Recommend deferring** it; it is not forced.
  - rmcp's client response cache: leave it on its default, which writes only when a server sends `ttlMs > 0` [D].
  - Sending our `client_info`: out of scope (§3.3).
  - **Which protocol version is offered.** (A)–(C) all take rmcp's default offer: 2025-11-25 from P20 on. **(d)** would instead pin the offered version, for example to today's 2025-03-26.
    - How: override `NotifyingHandler::get_info` to return a `ClientConfig` whose `protocol_version` is pinned. rmcp sends `service.get_info()` as the `initialize` params, and `ClientConfig` is `InitializeRequestParams` with a `protocol_version` field [D: `rmcp-3.4.1/src/service/client.rs:777`, `:894-903`; `model.rs:1031-1036`, `:1158`].
    - The same override is how `client_info` would be sent.
    - Not measured: whether rmcp 3.4.1's other behaviour, such as the `MCP-Protocol-Version` header, follows a pinned older version [NE].
    - Recommended only if P22's real-server round trip finds a server that rejects 2025-11-25 (Appendix B #4). Otherwise take the default.
- **Recommendation: (A).** P22's gate includes a real MCP server round trip.

**DU-6 — infer 0.22's new refusals of text uploads** (the documents group's D1).
- (a) Accept: some ordinary notes (a `.txt` that mentions `%PDF-1.7`, a CSV starting `AC1015,`) get `415` or are dropped on connectors.
- (b) Keep today's verdict for declared `text/*` with a narrow rule, P17: ignore a PDF detection whose `%PDF` is not at offset 0, and a DWG detection. P17 lands before the bump as a no-op under 0.16.
- (c) Hold infer at 0.16: no advisory, but the MP3/HEIF/prefixed-PDF acceptance gains are lost.
- **Recommendation: (b).**
- **The two media cases (connector path only, OK → BLOCKED [M]):** `audio/mp3` carrying MPEG-2 frames, and `image/heic` with brand `heix`.
  - (m-a) Accept. Such a connector attachment is dropped as a MIME mismatch.
  - (m-b) Extend the sanitizer's existing alias table, `is_audio_mime_compatible` [C: `sanitizer/mod.rs:268-282`, used by both paths], with `audio/mp3` ↔ `audio/mpeg`. This lands as a sibling of P17, before P18, with a P14 case. Side effect [inferred, not measured]: an `audio/mp3` declaration with MPEG-1 frames, which 0.16 already detected as `audio/mpeg` and refused, would be accepted too. That is a change beyond the bump.
  - (m-c) Also treat declared `image/heic` as compatible with detected `image/heif`. This accepts the `heix` case, and it **also** accepts every ordinary iPhone HEIC, which both versions refuse today [M]. That is a behaviour change well beyond the bump. The route then decides whether a provider can take a HEIC image part, and that is [NE].
  - **Recommendation: (m-b) now; (m-c) only as its own decision**, outside this wave. How often either case occurs is [NE].

**DU-7 — zip features** (the documents group's D2).
- (a) `["deflate"]`: every spec-conforming Office or iWork file reads identically; −23 lock entries, including four C libraries.
- (b) `["deflate", "deflate64", "bzip2", "lzma", "xz", "zstd"]`: keeps off-spec entries readable, for +11 packages including zstd's C library.
- **Recommendation: (a).**

**DU-8 — pdf-extract's Form-XObject font regression** (the documents group's D3).
- (a) Accept. Add the correct-text test marked `#[ignore]` with a pointer to an upstream issue, and file the issue with the p02/p03 fixtures.
- (b) Hold 0.8.2: keeps a daemon-killing DoS. Not recommended.
- (c) `[patch]` a fork with per-XObject font scope: a maintenance burden.
- No released pdf-extract has lopdf ≥ 0.42 without the regression [M: index].
- **Recommendation: (a).**

**DU-9 — C0 control characters and separator collisions in XLSX text.**
- Facts:
  - calamine 0.36 unescapes every `_x00HH_`, so C0 controls can enter `extracted_text` [D].
  - The extractor joins cells with TAB and rows with LF [C: `extraction.rs:101-104`]. A cell's own tab, which `_x0009_` now produces [M], reads as a cell boundary. A cell's own newline reads as a row boundary; for literal newlines that is pre-existing [M, §2.1].
- (a) Accept both. Plain-text uploads already store C0 as-is [C: `extraction.rs:52-58`], and the CLI never prints `extracted_text` [C: grep, 0 hits].
- (b) Strip C0 other than TAB, LF and CR in `extract_spreadsheet`.
- (b') In `extract_spreadsheet`, replace TAB, LF and CR **inside a cell** with a space before joining. This keeps the table's cells and rows as the file has them. It also changes the pre-existing literal-newline case, which is a small behaviour change beyond the bump. It is one line at `extraction.rs:102`, in P16 or a sibling `fix:` commit, with a P14 case for a tab and a newline inside a cell.
- **Recommendation: (a) for C0, recorded in P16's commit body; (b') for the separators**, because the model otherwise reads a changed table structure.

**DU-10 — Gate for keyring 4.**
- (a) First, the owner counts the lines matching `secret_ref` and `use_keychain = true` in **every** real `llm.toml`: a count only, no values printed (`grep -c`).
  - The files to check:
    - `~/.openalpaca/config/llm.toml`, which the app and `openalpaca daemon start` use;
    - the checkout's own ignored `config/llm.toml`, which a daemon started from `target/…` or with `cargo run` finds by walking up from the executable (exists [M: `ls`]; order [C: `bootstrap/config.rs:14-61`]);
    - the `llm.toml` under any other directory the owner points `OPENALPACA_CONFIG_DIR` at;
    - any copy an installer placed elsewhere.
  - If every count is 0, the bump touches no existing secret: P12 lands on CI plus the source evidence.
  - Otherwise P12 waits for a round trip on a **disposable** macOS user or VM. The HEAD build writes an entry with `use_keychain = true`; the new build must read it back.
- (b) Hold keyring 3.6.3: no advisory, but CI keeps `libdbus-1-dev`.
- Windows stays unverified under either answer.
- **Recommendation: (a).**

**DU-11 — tower-http 0.7 and tokio-tungstenite 0.30, or the dedup picks?**
- tower-http: 0.7.1 (a 0.6/0.7 duplicate in the daemon until reqwest moves), or hold 0.6.11.
- tokio-tungstenite: 0.30 (no binary links both versions), or 0.29 (−8 packages, and it must be re-bumped when axum moves).
- **Recommendation: 0.7.1 and 0.30.** The wave's goal is latest stable, and duplicates cost compile time only.

**DU-12 — teloxide over rustls.**
- (a) `default-features = false`, `features = ["macros", "ctrlc_handler", "rustls-native-roots"]` (P28): the OS trust store; no OpenSSL on Linux.
- (b) The `rustls` feature: bundled webpki roots, which ignores OS-installed corporate CAs.
- (c) Keep native-tls.
- **Recommendation: (a), if a live bot start and one message round trip can be run under the §5.2 isolation rule; otherwise (c) for now.** The TLS handshake itself was measured [M].

**DU-13 — Bun version and lockfile format.**
- Facts:
  - Bun 1.4 writes lockfile v2 for any lock it creates from scratch [Docs, fetched].
  - Dependabot's bun updater bundles Bun 1.3.14, which reads v1 only, and since PR #15896 it rejects a lock version it cannot read [Docs, fetched 2026-09-25; §2.10].
  - CI's Bun 1.4.2 installs a v1 lock under `--frozen-lockfile` [M: CI log].
- (a) Pin 1.4.2 with `packageManager` in the root `package.json` (P01), and regenerate `bun.lock` with 1.4.2 in P31. It becomes lockfile v2, so every local Bun must then be ≥ 1.4. **Under DU-15 (a), every Dependabot bun run then fails** until Dependabot ships Bun ≥ 1.4.
- (b) Pin 1.4.2, but commit the v1 lock regenerated from scratch by Bun 1.3.x. The researched locks are exactly that: Bun 1.3.13 [M].
  - P31–P33 are authored before the owner's local Bun is upgraded, or with any 1.3.x.
  - After that, local Bun is installed at exactly 1.4.2 (§5.1 item 2).
  - Every later lock change must keep `"lockfileVersion": 1`. Whether Bun 1.4.2 keeps a v1 lock at v1 when it re-saves it is [Docs, second-hand; not measured], so P35's gate and the §8 checklist check the version on every lock change.
  - Optional: a one-line CI assertion, such as `grep -q '"lockfileVersion": 1,' bun.lock` in the gui job. That is a new enforcement point, so it is part of this decision, not a default.
- (c) Pin 1.3.14, the version Dependabot bundles. Local, CI and Dependabot then agree, but CI steps back from 1.4.2, and the wave does not reach the latest Bun.
- Node: leave unpinned.
- **Recommendation: (b) while DU-15 is (a); (a) otherwise, or once dependabot-core's `bun/Dockerfile` pins Bun ≥ 1.4.** Moving to v2 later is its own small commit: regenerate the lock with the pinned Bun and remove the assertion. Under (a) or (b), re-run the four gates under Bun 1.4.2 before committing; that was not measured.

**DU-14 — Fresh JS majors, and the plugin swap.**
- Majors: (a) Vite 8 and Vitest 5 in this wave, at the newest 8.3.x / 5.0.x that is at least 7 days old when the branch is cut, matching Dependabot's cooldown. (b) Wait a release cycle. At research time 8.3.1 was 1 day old and 5.0.2 0 days.
- Swap `@vitejs/plugin-react-swc` for `@vitejs/plugin-react` 6.1.1 (P33): bundle byte-identical; removes `@swc/core` and 12 native binaries (16 packages) [M].
- **Other webviews.** Vite 8's Lightning CSS output (§2.10) is checked in Chromium [M] and will be checked in WKWebView (E5). It is not checked in WebView2 (Windows) or webkit2gtk (Linux), although the bundle targets include both [C: `tauri.conf.json`].
  - (w-a) Accept that risk explicitly in this decision.
  - (w-b) Run E5b: the Chat, Settings and Memory screens in a Windows and a Linux build, compared with the pre-wave build.
- **Recommendation: (a), and take the swap; (w-a) unless a Windows or Linux desktop is at hand, then (w-b).**

**DU-15 — Maintenance bot.**
- (a) Dependabot:
  - cargo `lockfile-only` with `allow: all` in one grouped PR (the semver-compatible lock refresh, transitive crates included); bun minor/patch grouped, majors ignored; github-actions moving the SHA pins;
  - monthly, with a 7-day cooldown;
  - the owner enables Dependabot alerts in the repo settings.
- (b) Renovate: needs a third-party app with write access, or self-hosting.
- (c) A manual quarterly chore. Then pin actions by major tag (`@v7`, `@v2`), not SHA, so fixes still flow in.
- **Sub-question under (a): the Tauri JS/Rust pairing.**
  - `tauri = "2"` and `tauri-plugin-dialog = "2"` [C: `src-tauri/Cargo.toml:27`, `Cargo.toml:150`] let a lockfile-only cargo PR move tauri to a new minor. Meanwhile `@tauri-apps/api` stays pinned exactly at 2.11.1 [C: `package.json:24`], and a separate bun PR moves it.
  - CI compiles nothing under `src-tauri`, and the Tauri CLI's own pairing check compares nothing under Bun [M, §2.10]. So a mismatched major.minor pair would merge green.
  - (p-i) Dependabot `ignore` for `tauri*` with `update-types: ["version-update:semver-minor"]` in the cargo entry, and for `@tauri-apps/*` minors in the bun entry. Patches still flow, and patch drift is allowed. Tauri minors are then moved by hand, in one commit that moves both sides (§8). No new CI check is needed. That `ignore` applies to a `lockfile-only` cargo update is [inferred from docs, not verified]; P35's gate checks it on the first run.
  - (p-ii) A CI check comparing the `tauri*` versions in `Cargo.lock` with `@tauri-apps/*` in `bun.lock`, in about ten lines. It is a new enforcement point.
  - (p-iii) A Dependabot group spanning both ecosystems. Whether Dependabot supports this for cargo + bun was not checked [NE].
- **Recommendation: (a) with (p-i).** SHA pins only together with (a).

**DU-16 — Pester.**
- Exactly 6.2.0 (nothing it removed is used [C]; not run [NE]), or exactly 5.9.1.
- **Recommendation: 6.2.0, falling back to 5.9.1** if P01's Windows job fails.

**DU-17 — Clippy cleanup and a warnings gate.**
- (a) Skip in this wave. CI gates no warnings, and the bump adds 17 to 80 older ones.
- (b) P36 only: 13 mechanical fixes plus 3 structural sites. Keep `event_bridge.rs:37`'s explicit `created_by: _` with an `#[allow]`, since CLAUDE.md says it is ignored on purpose (R79).
- (c) P36 plus a warnings gate (Cargo 1.97 `build.warnings`, or `-D warnings`). This first needs the 80 older warnings and the 2 Linux-only ones fixed.
- **Recommendation: (a).** Revisit (c) as its own piece of work.

**DU-18 — Windows coverage.**
- (a) A temporary `windows-latest` job on the wave's PR that runs `cargo check --locked --workspace --exclude openalpaca_gui`. It would be the first Windows compile of keyring's windows store and of everything else.
- (b) Make that job permanent.
- (c) None: the first Windows compile is then the owner's next Windows release build.
- (a) is carried by two commits: **P01a** adds the job, and **P40** removes it before the merge. The job is a PR-time measurement, not a standing rule. A `workflow_dispatch` workflow run by hand on the branch would do the same without the removal commit.
- Not established: whether the runner builds aws-lc-sys, which reqwest 0.13 pulls in, without an extra step [NE]. Its build script asks for NASM, or for `AWS_LC_SYS_PREBUILT_NASM` to be set, when it cannot assemble [D: `aws-lc-sys-0.45.0/builder/cmake_builder.rs:688-695`]. If P01a fails there, rather than in our code, set that variable or install NASM in the job. That is a runner fact, not a bump regression.
- **Recommendation: (a).**

**DU-19 — Repository hygiene (P37).**
- Remove `rust_out` and `pnpm-workspace.yaml`; add `--locked` and `--frozen-lockfile` to the release scripts.
- **Recommendation: take it.**

**DU-20 — Should migrations avoid SQL an older build's SQLite cannot parse?**
- The clean refusal survives the new `ALTER TABLE` forms [M]. A 3.53-only function in an index would only break writes in an older build, which refuses a newer schema before any write anyway [C: `database/mod.rs:178`; M].
- **Recommendation: no rule.**

**DU-21 — An advisories gate (P34), and who answers it.** CLAUDE.md: a default that adds an enforcement point is a decision. P34 is one.
- Facts:
  - At HEAD, 5 RustSec vulnerabilities (2 reachable, 2 likely reachable) and 10 `bun.lock` advisories (7 high) sat unnoticed [M].
  - Dependabot alerts are off [M]. Dependabot does no security updates for bun [Docs: CI report].
  - `bun audit` exits **1** on the vulnerable baseline lock ("10 vulnerabilities (7 high, 3 moderate)") and **0** on the re-resolved one [M: `SP/logs/rev2-bun-audit-{baseline,fresh}.txt`, Bun 1.3.13].
  - It has `--audit-level=<level>` and `--ignore=<CVE>` [M: `bun audit --help`].
- (a) Adopt P34 as written, plus a `bun audit` job in the same `audit.yml`: setup-bun by SHA, `bun install --frozen-lockfile`, `bun audit`. It runs on PRs, on pushes to main, and weekly (Monday 06:17 UTC).
  - RustSec policy: `yanked = "deny"`, `unmaintained`/`unsound = "workspace"`, exceptions only as reasoned `ignore` entries.
  - bun: exceptions only as `--ignore=<CVE>` with a comment.
  - **The owner answers a red weekly run.** The remedy is a lock refresh, a bump, or a reasoned exception. A red PR run on an unchanged lock means a new advisory, not a fault in the PR.
- (b) Report only: the weekly run with `continue-on-error`, and no PR runs. Nothing blocks, and a red run is easy to miss.
- (c) No gate. Rely on Dependabot alerts, cargo only, once turned on.
- **Recommendation: (a).** P34 is **Recommended (DU-21)** in rev 2, no longer Required.

**DU-22 — `--locked` in CI (P01).**
- `--locked` makes a CI build fail when `Cargo.lock` is out of date with the manifests, instead of silently resolving a different lock. It is a new enforcement point [C: CI runs without it today].
- The release scripts get the same flag (P37, DU-19).
- (a) Adopt. (b) Leave CI as it is.
- **Recommendation: (a).** Every commit in this wave already produces its lock with cargo, in sequence (§5.2).

---

## 5. Commit plan

### 5.1 Before the first commit: owner actions

1. Take DU-1 to DU-22 (at least DU-1, 2, 3, 4, 6, 10, 13, 21).
2. Install local Bun at **exactly** the pinned version: `curl -fsSL https://bun.com/install | bash -s "bun-v1.4.2"`. `bun upgrade` installs the newest release, not the pin [Docs: bun.com/docs/installation, fetched]. Under DU-13 (b), do this **after** P31–P33 have regenerated `bun.lock` with Bun 1.3.x.
3. The DU-10 count, and the E6 no-print checks (§6) of every real config file and skill directory. They are needed before **P04** (toml_edit 0.25.15 arrives there), P12, P29 and P30.
4. **Backups before the first start of a bumped build on the real store.** Stop the real daemon, then copy:
   - `state/openalpaca.db` with its `-wal` and `-shm` files;
   - every real `config/*.toml` (both copies, DU-10);
   - `plugins/.permissions.toml`;
   - `plugins/.config/`.

   The config files are included because files written after the bump go through new code: the M3 `llm.toml` edit uses toml_edit 0.25.15 (P04), and `openalpaca config set|unset daemon.*` rewrites `daemon.toml` whole under toml 1.1 (P29) [C: `daemon_config_cli/mod.rs:408, :438`]. The standing advice in CLAUDE.md applies. This wave adds no migration, and the database files are portable both ways [M], so the copies are insurance. Then run **E8** (§6) on a copy before that first start.
5. **One freshness rule for both ecosystems.** Every target version, Rust or JS, is at least 7 days old when the branch is cut, matching Dependabot's cooldown. Otherwise take the newest release that is, and re-run that commit's gate.
   - At research, rmcp 3.4.1, lru 0.18.5 and thiserror 2.0.21 were 2 days old (all 2026-09-23 [M: `SP/direct-deps.tsv`, from the crates.io API]). So was process-wrap 10.0.1, which arrives with P22 [D: its CHANGELOG]. Vitest 5.0.2 was 0 days old, and Vite 8.3.1 1 day.
   - Also re-run `cargo update --dry-run --verbose` and `bun outdated`. A newer *patch* of a researched target goes through that commit's gate. A new *major* (e.g. rmcp 4, toml 2) is outside this plan's evidence: pin the researched major, or research the new one first.
6. **G-base, the wave's first gate, on the exact branch point.**
   - Apply `SP/patches/combined.diff` (or its preserved copy, Appendix A) to an export of the branch point.
   - Run `cargo +1.98.1 check --locked --workspace --all-targets --exclude openalpaca_gui`, then the **full** suite (G-CI's Rust part) and `python3 scripts/gen_api_docs.py --check`.
   - Expect 0 errors, 0 failed (apart from the known flake, which must pass on rerun), and only the known `docs/api/crates/openalpaca_mcp.md` staleness that P21 fixes.
   - This proves the set still combines at the tip. It was last done at `908050f1` for `check` and the nearest suites only [M, §0].
   - A conflict or failure there means re-cutting with the replay driver (Appendix A) before P01.
7. **Record the wave in `tasks/todo.md`** as the next chunk (Chunk 25 at rev 2 [C: the last heading is Chunk 24]), per CLAUDE.md's Task Management: the commits as checkable items, marked as they land, and a review section at the end with the E1–E8 results. `tasks/todo.md` is git-ignored [M: `git check-ignore`], so it is a local record, not part of a commit.

### 5.2 Conventions for every commit

- **Messages:** conventional prefix. Every substantial commit ends with a `Verified:` line naming the gate results, and carries the attribution lines the repository uses.
- **Docs in the same commit:** a doc line that cites a version changes in the commit that changes the version (§2.13 row 13).
- **The lock is produced by cargo, in sequence.** Edit the manifest line and let cargo regenerate `Cargo.lock`. Never apply a group's lock hunk: every group cut its lock against HEAD, so the hunks do not stack [M, integrator]. `SP/patches/combined-commits/NN-*.diff` are the reference results.
- **Test hygiene:**
  - run tests with a sandboxed `HOME`, `OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR`, so nothing reaches `~/.openalpaca` or the keychain. With `HOME` overridden, also set `RUSTUP_HOME` and `CARGO_HOME` to their real values so cargo still finds its toolchains, as rev 2's runs did;
  - redirect output to a file, never pipe `cargo test` into `head`/`grep`, and count warnings from colour-free logs. `.cargo/config.toml` sets `[term] color = "always"` [C], so colour-free means passing `CARGO_TERM_COLOR=never` explicitly;
  - after a daemon test run, kill only the orphaned `sleep 3600` children with ppid 1 that started inside that run.
- **Isolation of every manual daemon, CLI or app run** (P04's and P22's MCP checks, P26, P27, P28, P32's `tauri dev`, E4, E5, E7, E8). Being wrong here writes the owner's real config, keys or projects. `tasks/lessons.md:49` already says: never run the daemon without both `OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR` in a temp directory.
  1. **Store, config and home.** In one shell, `T=$(mktemp -d)`, then export:
     - `OPENALPACA_HOME_STORE=$T/store`;
     - `OPENALPACA_CONFIG_DIR=$T/store/config` (create the directory first: a value that does not exist is ignored with a WARN [C: `bootstrap/config.rs:14-26`]);
     - `HOME=$T/home`;
     - `RUSTUP_HOME`/`CARGO_HOME` at their real values.

     Without `OPENALPACA_CONFIG_DIR`, a daemon started from the checkout (`cargo run -p openalpacad`, `target/*/openalpacad`) finds the checkout's real, ignored `config/llm.toml` by walking up from its executable. It would read that file, and write a key added in the run into it, encrypted under the temp store's `.master_key` [C: `bootstrap/config.rs:14-61`; M: the file exists]. The app and `openalpaca daemon start` set the config dir to `<store>/config` themselves [C: `src-tauri/src/lib.rs:233-271`, `apps/openalpaca/src/manager.rs:219`].
  2. **Credential discovery off.** Before the first start, write `$T/store/config/llm.toml` from `scripts/release/templates/config/llm.toml`, plus `[credential_discovery]` with `claude_code = false` and `codex = false`.
     - Otherwise every daemon start runs discovery. With `HOME` redirected, discovery falls back to `security find-generic-password` on the **real** login keychain, and it may import the owner's Claude Code token into the test daemon [C: §3.3].
     - The daemon's seeding writes `llm.toml` only when it is absent [C: `bootstrap/config.rs:84-96`, `seed_default_configs`].
     - Keep `[security] use_keychain` unset, so false.
  3. **Working directory and project.**
     - Run every CLI command from a scratch directory **outside any git repository**, or pass `--global`. The CLI sends its working directory as the project (`x-workspace-path`) and memory verbs scan its marker root [C: `apps/openalpaca/src/commands/memory.rs:99-137`]. So a run from the checkout writes project memory, and later learned notes, into `<checkout>/.openalpaca/`, which this repository does not git-ignore [M: `git check-ignore` exits 1].
     - In the app, the project comes from WebKit `localStorage` key `oa-project` [C: `stores/project.ts:22-69`]. WebKit keeps that storage per app. `~/Library/WebKit/com.tangyunpei.openalpaca-gui` exists on this machine, and so does `~/Library/WebKit/openalpaca_gui` [M: `ls`]. That a bundled app's storage is keyed by its bundle identifier is inferred from those names; which run uses which directory is [NE].
     - So E5 builds with a **throwaway identifier** (§6). In any app run, confirm before the first write, chat turn included, that no project is selected, or that a scratch directory outside any repository is. A read opens a project partition read-only [C: `memory_files/partition.rs:530-580`]. But the background learner writes each user turn's notes into that turn's own project root.
  4. **Which daemon binary.**
     - `openalpaca daemon start` launches, in order: `OPENALPACA_DAEMON_BIN`, a daemon next to the CLI, `../libexec`, then the first `openalpacad` on `PATH` [C: `manager.rs:241-284`]. So a CLI built without its daemon can start an installed, pre-bump one. Set `OPENALPACA_DAEMON_BIN` to the binary under test.
     - For the app, see G-ident.
  5. **Environment reaches the app only from the shell.** An `.app` opened from Finder or with `open` does not inherit the shell's variables. Start its executable directly: `<app>/Contents/MacOS/<exe>`, the main binary, `openalpaca_gui` per CLAUDE.md; `ls` the directory to confirm.
  6. **Confirm before any write.**
     - `GET /v1/status` → `home_root` is `$T/store` [C: `routes/status.rs:79, :315`]. For the app, Settings → Connection shows the same root.
     - The daemon log names the config dir it resolved (`Config base dir: …` [C: `apps/openalpacad/src/main.rs:137`]); its path is `/v1/status`'s `log_path` for a launcher-managed daemon.
  7. **Cleanup.** Delete `$T` when done. It holds a `.master_key` and any key typed in the run. For E5, also delete the directories named after the throwaway identifier under `~/Library` (`find ~/Library -maxdepth 2 -name '<id>*'` lists them).

**Named gates**, referred to below:

| Gate | What it is |
|---|---|
| **G-check** | `cargo +1.98.1 check --locked --workspace --all-targets --exclude openalpaca_gui` (`+1.93.0` before P03). Expect 0 errors; the only warning is the `proc-macro-error2` future-incompat note from P03 on. |
| **G-CI** | What CI runs, locally and on the PR: `cargo build --locked` and `cargo test --locked` (`--workspace --exclude openalpaca_gui`); `cargo clippy --locked --workspace --exclude openalpaca_gui --all-targets`; `bun run check`, `test`, `format:check`, `build`; Pester. Locally also `python3 scripts/gen_api_docs.py --check`: `docs/api/` is generated and never hand-edited, and CI does not run the check. Pass means 0 failed tests and a clean `--check`. A failure in `managers::mcp::tests` counts as the known pre-existing flake only if that module passes on a rerun. |
| **G-GUI** | By hand, because CI compiles nothing under `src-tauri`: `cargo check -p openalpaca_gui` and `cargo clippy -p openalpaca_gui --all-targets`, **with a `CARGO_TARGET_DIR` of its own**, never the one a later step runs or bundles `openalpacad` from. tauri-build's build script copies `src-tauri/bin/openalpacad-<triple>` to `<target dir>/<profile>/openalpacad` [D: `tauri-build-2.6.3/src/lib.rs:41-75, :535-551`; M: the research target dirs `SP/target-tc/debug/openalpacad` and `SP/target-tc193/debug/openalpacad` are 0-byte copies of a placeholder]. The build script needs that path to exist [M: toolchain report B6]. **Where the real sidecar exists, as in the main checkout (54,894,912 bytes [M]), create nothing.** Only in a fresh export or worktree where the path is absent, create an empty placeholder there, and delete it afterwards. Never create one over a real sidecar: an empty file with a newer mtime makes `prepare-sidecar.ts` skip its build, and the next `tauri dev` or `tauri build` bundles an empty daemon [C: `prepare-sidecar.ts:45-56`]. Expect 0 warnings in the crate; the 2 `let_and_return` in `openalpaca_storage/src/artifacts/mod.rs:1860, :1945` are pre-existing. Needed after P04, P08, P12 and at the end, since the GUI links `openalpaca_storage`. |
| **G-lint** | Clippy unique-location count and `cargo fmt --all -- --check` hunk count against the previous commit. Measured anchors: 97 unique after P03, 96 at the end. **Every commit leaves the fmt hunk count equal to its parent's** (1456 at the base [M]). So rustfmt's suggestion is applied to each commit's added lines in that commit. The replay needed this in P05 (the fixture test), P20–P22 (the rmcp lines; the replay deferred them to P22) and P30 (the serde-saphyr pin test) [M: integration report §5.3]. New commits (P14, P17, P24, P36) follow the same rule. |
| **G-ident** | For any packaged or bundled build (E5, E5b), and for E7's pre-wave binaries: the daemon inside the bundle is the one built from the commit under test. `prepare-sidecar.ts` can silently reuse a stale or debug copy (§3.3); the workspace version stays 0.1.0, so nothing on screen shows which daemon runs. The check: `shasum -a 256` of the bundled `Contents/MacOS/openalpacad*` equals that of the `target/release/openalpacad` built from the commit, **and** the bundled daemon carries that commit's version markers: `LC_ALL=C grep -c -a -F <marker> <binary>`. After the wave: `rmcp-3.4.1` ≥ 1, `rmcp-0.16.0` = 0, `3.53.2` ≥ 1. Before it: `rmcp-0.16.0` ≥ 1 and `3.51.1` ≥ 1. Measured: the main checkout's pre-bump release sidecar has `rmcp-0.16.0` ×7, `3.51.1` ×1, and no new marker; the combined state's debug build has `rmcp-3.4.1` ×18, `3.53.2` ×1, and no old marker [M]. That registry crate paths survive into release binaries, through panic locations, is shown for the old markers by the release sidecar; for the new markers in a release build it is [inferred], so E5 checks it. |

**Backing out, in general:**
- Revert in reverse order.
- To revert a middle commit, restore its manifest line and run `cargo update -p <crate> --precise <old version>` if the lock conflicts; then run that commit's gate.
- The data formats are safe to roll back where marked **[rollback M]** below, i.e. the old build reads what the new one wrote (measured).

"Replay NN" below refers to `SP/patches/combined-commits/NN-*.diff`, the integrator's measured result for that commit.

### 5.3 The commits

Status in each heading:
- **Required:** lands under every DU answer except one that removes it.
- **Recommended (DU-x):** lands if DU-x goes the recommended way.
- **Optional (DU-x):** does not land by default.
- **Conditional:** lands only if its stated condition holds (P01b, P39).

#### Phase A: CI and repository (no Rust change)

**P01 — `ci: pin actions by SHA; take the Rust toolchain from rust-toolchain.toml; pin Bun`** · Required · replay 01
- **Contents.** In `.github/workflows/ci.yml`:
  - `permissions: contents: read`;
  - `actions/checkout@3d3c42e5… # v7.0.1` with `persist-credentials: false`, in all three jobs;
  - the `dtolnay/rust-toolchain@master` step and its `toolchain: 1.93.0` are replaced by `rustup toolchain install` + `rustup show active-toolchain`;
  - `Swatinem/rust-cache@6323deb1… # v2.9.2`;
  - `--locked` on build, test and clippy;
  - `oven-sh/setup-bun@0c5077e5… # v2.2.0`;
  - Pester is required at exactly 6.2.0.

  Root `package.json` gets `"packageManager": "bun@1.4.2"`. Docs: `CLAUDE.md:152` names the Bun pin; `CLAUDE.md:154` says the toolchain comes from `rust-toolchain.toml` and CI runs `--locked`.
- **Prerequisites.** DU-13, DU-15 (without Dependabot, use `@v7`/`@v2` tags, not SHAs), DU-16, DU-22 (without it, drop `--locked`).
- **Gate.** Already measured locally: `git apply --check`, the YAML parses, zizmor 0 findings, and `bun install --frozen-lockfile` under Bun 1.3.13 with the field present leaves the lock untouched [M]. On GitHub, all three jobs green:
  - the rust log shows "overridden by rust-toolchain.toml" and 1.93.0;
  - the gui log shows `bun install v1.4.2` passing on the v1 lock;
  - the Windows log shows Pester 6.2.0 and "Tests Passed: 15";
  - no Node 20 annotation.
- **Back-out.** Revert. If only the Pester job fails, set it to 5.9.1.

**P01a — `ci: temporary Windows cargo check for the toolchain and dependency wave`** · Recommended (DU-18 a) · new
- **Contents.** One job in `ci.yml`:
  - `runs-on: windows-latest`, `permissions: contents: read`;
  - checkout by SHA with `persist-credentials: false`, then `rustup toolchain install`, then `Swatinem/rust-cache` by SHA;
  - `cargo check --locked --workspace --exclude openalpaca_gui`.

  The commit body says it is removed by P40 before the merge. Placed here, every later push of the wave gets a Windows compile: keyring's Windows store (P12), sysinfo (P08), rmcp's process-wrap 10 Windows job objects (P22), and the rest.
- **Gate.** The job runs. Until P03 it compiles the unbumped tree on 1.93.0, which is itself the first Windows Rust compile anywhere [NE until then]. A failure inside `aws-lc-sys`'s build script is a runner prerequisite, not a regression: see DU-18 for NASM and `AWS_LC_SYS_PREBUILT_NASM` [D]. A failure in our code before P03 is pre-existing, and is recorded, not fixed, in this wave.
- **Alternative.** A `workflow_dispatch` workflow run by hand on the branch. Then there is no P40.

**P01b — `ci: pin runs-on ubuntu-24.04 for the wave`** · Conditional (only if the PR will run between 2026-10-19 and 2026-11-19, when `ubuntu-latest` moves to 26.04 [M: CI notice]) · new
- **Contents.** `runs-on: ubuntu-24.04` for the rust and gui jobs, and for `audit.yml` once P34 lands. The commit body names P39 as its revert.
- **Gate.** CI green, with the image line showing 24.04.

**P02 — `chore: remove the src-tauri Cargo.lock that cargo never reads`** · Required · replay 02
- **Contents.** `git rm apps/openalpaca-gui/src-tauri/Cargo.lock`.
- **Gate.** From `apps/openalpaca-gui/src-tauri`: `cargo metadata --locked` exits 0 and the root lock is unchanged; `bunx tauri info` reports the root lock's tauri. Both were measured with the file corrupted.
- **Back-out.** Revert; harmless.

#### Phase B: toolchain

**P03 — `build: pin Rust 1.98.1; workspace rust-version 1.98`** · Required · replay 03
- **Contents.** `rust-toolchain.toml:2` `channel = "1.98.1"`; `Cargo.toml:32` `rust-version = "1.98"`; `README.md:26`, `README.md:208` and `CLAUDE.md:152` name 1.98.1. There is no lock change, no source change, and no `ci.yml` edit (P01 removed CI's copy).
- **Prerequisites.** P01; DU-1 (never while L8 builds from this code); DU-2; DU-3.
- **Gate.**
  - `cargo +1.98.1 metadata --locked`: the lock stays byte-identical [M].
  - G-check.
  - G-CI: all pass apart from the known flake [M: 4165/1/13 at `5e5ac317`].
  - G-lint: 97 unique clippy warnings (+17, the lints in §2.2), 0 rustc warnings, fmt output identical to the parent [M].
  - Exactly one future-incompat report, `proc-macro-error2` [M].
  - G-GUI.
- **Back-out.** Revert, but only while P04 and P08 are not in: takecell 0.1.2 and sysinfo 0.39.6 refuse 1.93.0 [M].

**P04 — `chore(deps): refresh Cargo.lock within semver (cargo update on 1.98.1)`** · Required · replay 04
- **Contents.** `Cargo.lock` only.
- **Researched result.** 80 packages (§2.3): rustls 0.23.45, tauri 2.11.6, tauri-plugin-dialog 2.7.3, tauri-plugin-fs 2.5.2, toml_edit 0.25.15, toml 1.1.6, encoding_rs 0.8.42 and more.
- **At branch time.** Compare `cargo update`'s list with that one. For every extra package on a data path, read its changelog. **The data-path watch-list**, reused by §8's per-PR checklist: confirm these are unchanged, or read what changed. libsqlite3-sys, rusqlite, aes-gcm/aes, sha2, blake3, keyring, serde, serde_json, chrono, unicode-normalization, regex/regex-syntax/regex-automata, toml*, encoding_rs, base64.
- **Gate.**
  - G-check; G-CI; G-lint (identical to P03 [M]); G-GUI (tauri 2.11.6 clean [M]).
  - cargo-audit: the rustls advisory is gone.
  - Manual, under the §5.2 isolation rule: one MCP stdio server through the daemon (process-wrap moved).
  - Before this commit, not after it: the E6 owner check of every real TOML file through toml_edit 0.25.13 and 0.25.15 (`tedit`, Appendix A), because from here on the M3 `llm.toml` edit renders through toml_edit 0.25.15 (§5.1 item 3).
- **Back-out.** Revert; this restores rustls 0.23.43 and its advisory.

#### Phase C: format pins

**P05 — `test(llm,storage): pin the on-disk secret and hash formats before the crypto bumps`** · Required · replay 05
- **Contents.**
  - `a_secret_written_by_the_aes_gcm_0_10_build_still_decrypts` in `crates/openalpaca_llm/src/keys/key_encryption/tests.rs`. The fixture `aes256:HFTMjzaU…` was written by HEAD's construction under key `00..1f` and decrypts to `sk-ant-api03-test-key-1234567890`.
  - A `content_io::sha256_hex(b"abc")` known-answer test in `crates/openalpaca_storage/src/content_io.rs`. It pins the `file_assets.sha256` spelling, which had no literal test.
  - The memory owner-key, partition-key and revision known answers already exist.
- **Gate.** Passes **before** the crypto bumps: key_encryption 14/14, content_io 1/1 [M, on the P04 base]. `cargo test -p openalpaca_llm -p openalpaca_storage`. Re-run after P10 and P11: 107 and 14 passed [M].
- **Back-out.** Revert; tests only.

#### Phase D: storage

**P06 — `chore(deps): rusqlite 0.40.2 (bundled SQLite 3.51.1 → 3.53.2)`** · Required · replay 06
- **Contents.** `Cargo.toml:97` `rusqlite = { version = "0.40.2", features = ["bundled"] }`; the lock moves rusqlite, libsqlite3-sys 0.38.2 and hashlink 0.12.2. The commit body names three things: the 17-digit REAL → TEXT change, which no SQL of ours reaches; the WAL-reset, lock-deadlock and EXISTS-to-JOIN fixes; and the schema-42 re-test.
- **Gate.** Pass means 0 failed; the counts are reference points measured outside the sequence.
  - G-check.
  - `cargo test -p openalpaca_storage`: 678 passed / 3 ignored on the storage group's tree [M]. In sequence expect 679, because P05 adds the `content_io` known-answer test (schema-42 refusal, INDEXED BY and query-plan tests, memory fixtures).
  - `cargo test -p openalpaca_core memory`: 335 [M].
  - `cargo test -p openalpaca_connectors --features imessage`: 70 [M] at `5e5ac317`. The tip has added connector tests since. At `908050f1`, the wider `--features telegram,discord,imessage` run gives 112/0 [M, rev 2]; the `imessage`-only count there was not taken.
  - G-GUI at the phase end.
  - Optional: the cross-version probe (`SP/probe`).
- **Back-out.** Revert. [rollback M: 3.51.1 opens files and WAL written by 3.53.2]

**P07 — `chore(deps): lru 0.18`** · Required · replay 07
- **Contents.** `Cargo.toml:143` `lru = "0.18"`.
- **Gate.** G-check; `cargo test -p openalpaca_core --lib compose`: 82 [M].
- **Back-out.** Revert.

**P08 — `chore(deps): sysinfo 0.39.6`** · Required · replay 08
- **Contents.** `Cargo.toml:94` `sysinfo = "0.39.6"`. Needs P03, because sysinfo requires Rust ≥ 1.95.
- **Gate.**
  - G-check.
  - `cargo test -p openalpaca_storage daemon_lifecycle`: 9, including the real unreaped-child test [M, macOS]. The PR's ubuntu job is the Linux measurement.
  - G-GUI.
- **Back-out.** Revert.

#### Phase E: secrets and hashing

**P09 — `build(deps): base64 0.23`** · Required · replay 09
- **Contents.** `Cargo.toml:100` `base64 = "0.23.1"`; lock edges only.
- **Gate.** G-check; `cargo test -p openalpaca_llm -p openalpaca_storage`; `cargo test -p openalpaca_core --lib -- update_persona memory`.
- **Back-out.** Revert. [rollback M]

**P10 — `build(deps): sha2 0.11 (digest 0.11, hybrid-array)`** · Required · replay 10
- **Contents.** `Cargo.toml:141` `sha2 = "0.11"`. 10 `format!("{:x}", …)` sites are rewritten to `{b:02x}`:
  - `crates/openalpaca_storage/src/content_io.rs:18`, `database/tests.rs:1097`, `store/tests.rs:440`;
  - `crates/openalpaca_core/src/tools/builtins/update_persona/common.rs:8`, `crates/openalpaca_core/tests/common/eval.rs:363`;
  - `apps/openalpacad/src/hot_reload.rs:43, 244, 279, 317`;
  - `apps/openalpacad/src/managers/mcp.rs:2435`.

  The commit body warns that `{:?}` of a digest compiles and changes its output.
- **Gate.**
  - G-check.
  - `cargo test -p openalpaca_storage`: owner key, partition keys, revision, HOME_README digest, schema-42, P05.
  - `cargo test -p openalpaca_core --lib -- memory update_persona session_log`: 476 [M].
  - `cargo test -p openalpacad -- persona hot_reload`: **not run by any group; run it here.**
  - G-GUI.
- **Back-out.** Revert. [rollback M: identical hex]

**P11 — `build(deps): aes-gcm 0.11 (aead 0.6)`** · Required · replay 11
- **Contents.** `Cargo.toml:139` `aes-gcm = "0.11"`; `crates/openalpaca_llm/src/keys/key_encryption/{mod,tests}.rs`.
- **Gate.** G-check; `cargo test -p openalpaca_llm`, including P05's fixture (key_encryption 14 [M]). Optionally re-run `SP/scratch-secrets/compat`.
- **Back-out.** Revert. [rollback M: the old code decrypts what the new code wrote]

**P12 — `build(deps): keyring 4.2 (v1 feature; Secret Service over zbus on Linux)`** · Required, gated by DU-10 · replay 12
- **Contents.** `Cargo.toml:148` `keyring = { version = "4.2", default-features = false, features = ["v1"] }`, with a comment on why the features are named. The lock gains keyring-core, the three platform store crates and the zbus stack (Linux only), and drops dbus-secret-service, security-framework 2 and core-foundation 0.9.
- **Gate.**
  - G-check (macOS).
  - The PR's ubuntu job: the first Linux compile of zbus.
  - DU-10: the owner's count is 0, or a disposable-keychain round trip passes.
  - G-GUI.
- **Back-out.** Revert. keyring 3 reads v4-created Linux items through its default-collection fallback [D]; on macOS and Windows they are the same items [D].

**P13 — `ci: the Rust job no longer needs libdbus-1-dev`** · Recommended · new
- **Contents.** `ci.yml:19-21` installs `pkg-config` only (aws-lc-sys needs it), with the comment corrected. `README.md:29` narrows dbus to the desktop app. It also adds the OpenSSL headers (`libssl-dev`) that the daemon's Linux build needs while teloxide uses native-tls (§3.3). If P28 lands, P28 removes `libssl-dev` again. CI needs no change for OpenSSL: CI is green today without installing it, so the image supplies it [M: CI log; which image package provides it is not checked].
- **Prerequisites.** P01, P12.
- **Gate.** A green ubuntu run without the package, and `cargo tree --target x86_64-unknown-linux-gnu --workspace --exclude openalpaca_gui -i libdbus-sys` prints nothing [M].
- **Back-out.** Revert.

#### Phase F: documents and upload sniffing

**P14 — `test(uploads): characterise extraction and sniffing on generated fixtures`** · Required · **new, not yet written**
- **Contents.** Tests in `extraction.rs`, the `routes/files` tests and the `sanitizer` tests, with no binary files committed:
  - XLSX cases:
    - untagged whitespace;
    - a rich run after a plain `<t>`;
    - CRLF;
    - `_x000D_`, `_x0009_`, `_x0000_`, `_x001B_`;
    - a tab and a newline inside one cell, pinning the separator behaviour of DU-9;
    - inline CDATA;
    - an `inlineStr` with a stray `<v>`, and a `t="s"` cell with `<v/>`;
  - a DOCX with a bzip2 entry;
  - `text/plain` and `text/html` bodies with `%PDF` in the first KiB, and a `text/csv` starting `AC1015,`;
  - the two connector media cases (DU-6): `audio/mp3` carrying an MPEG-2 frame (`FF F3`), and `image/heic` with brand `heix`. Today both pass the sanitizer;
  - an AES-128 PDF, expecting today's error;
  - the nested-array PDF, marked `#[ignore]` because on this tree it aborts the test binary.
- **How the fixtures are made.** Rev 1's "generated inside the tests" does not hold for two of them.
  - **Built in the test with what the tree already has:**
    - the XLSX workbooks, zipped with the `zip` crate the daemon already depends on [C: `apps/openalpacad/Cargo.toml:47`], using stored or deflated entries;
    - the text, CSV and HTML bodies;
    - the MP3 and HEIC headers, 64 and 56 bytes;
    - the nested-array PDF, a repeated `[` pattern around a minimal PDF.
  - **Not buildable in the test:**
    - The AES-128 PDF needs MD5 and AES-CBC, which are not dependencies of the daemon. The documents group made it with lopdf 0.42's `encrypt` [M: `SP/scratch-docs/cmp/src/main.rs:164-201`], an API P14's tree (lopdf 0.34) does not have.
    - A bzip2 DOCX can no longer be written with `zip` once P16 drops its `bzip2` feature. The documents group used Python's `zipfile.ZIP_BZIP2` [M: `SP/scratch-docs/make_fixtures.py:239`].
    - Choice (a), recommended: embed both as **byte literals in the test source**, produced once from `SP/scratch-docs/fixtures/e01_p01_encrypted_v4.pdf` (1,380 B) and `d02_bzip2_entry.docx` (898 B) [M: `ls`]. A hex-escaped `&[u8]` needs no new dependency; the daemon has no `base64` dependency [C: `apps/openalpacad/Cargo.toml`]. Record in a comment how each was made. Preserve those two files (Appendix A).
    - Choice (b): dev-dependencies. That means lopdf 0.42 for encryption, which clashes with the 0.34 that P14 runs against, and zip's `bzip2` feature, which feature unification would switch on for test builds. Both would need to be listed in the commit with their lock delta. Not recommended.
- **Effort** [inferred]: about 200–300 lines of test code and two hex-escaped byte literals of about 5.5 KB and 3.6 KB of source. It is small, but it is the one unwritten Required commit.
- **Gate.** Green with today's expectations: `cargo test -p openalpacad -- extraction:: routes::files`; `cargo test -p openalpaca_core -- sanitizer`. rustfmt-clean for its added lines (G-lint).
- **Back-out.** Revert.

**P15 — `fix(deps): pdf-extract 0.12 / lopdf 0.42 (RUSTSEC-2026-0187)`** · Required · replay 13
- **Contents.** `Cargo.toml:145` `pdf-extract = "0.12"`. Tests:
  - the nested-array test is no longer ignored;
  - the AES expectation becomes the extracted text;
  - a Form-XObject test asserts the **correct** text, `#[ignore = "pdf-extract 0.12 regression, upstream <issue>"]` (DU-8), and the upstream issue is filed.

  The commit body names: text changes for some real PDFs (the Hindi licence, now matching PDFKit); retryable `Error` rows will be re-extracted; the `println!` noise leaves `daemon.log`.
- **Gate.** G-check; `cargo test -p openalpacad -- extraction::`; cargo-audit without the lopdf advisory (the new `ttf-parser` unmaintained warning is expected); G-CI.
- **Back-out.** Revert. This reopens the DoS; rows extracted in the meantime keep their text.

**P16 — `chore(deps): calamine 0.36 + zip 8, deflate only (RUSTSEC-2026-0194/0195)`** · Required · replay 14
- **Contents.** `Cargo.toml:146` `zip = { version = "8", default-features = false, features = ["deflate"] }` (DU-7); `Cargo.toml:147` `calamine = "0.36"`. P14's XLSX and bzip2-DOCX expectations are updated. The commit body names the cell-text changes, C0 characters (DU-9), and the switch of flate2's backend.
  - Under DU-9 (b'), `extraction.rs:102` maps TAB, LF and CR inside a cell to a space, here or as a sibling `fix:` commit just before. P14's in-cell case then expects cells and rows exactly as the file has them.
  - Under DU-9 (a), P14's case is updated to the collided text, and the commit body says so.
- **Gate.** G-check; `cargo test -p openalpacad -- extraction:: routes::files`; `cargo tree -p openalpacad -i zip` shows one zip; cargo-audit without the quick-xml advisories. No quick-xml ≤ 0.40 remains; 0.41 and 0.42 coexist, as expected (§2.13 row 8).
- **Back-out.** Revert.

**P17 — `fix(uploads): a declared text file is not refused for a PDF or DWG signature past its first bytes`** · Recommended (DU-6 b) · new
- **Contents.** In `validate_magic_mime` (`apps/openalpacad/src/routes/files_types.rs:18`) and in `InputSanitizer::validate_upload_with_image_limit` (`crates/openalpaca_core/src/security/sanitizer/mod.rs:215`): for a declared `text/*`, ignore an `application/pdf` detection whose `%PDF` is not at offset 0, and an `image/vnd.dwg` detection. It lands before the bump, as a no-op under infer 0.16, so P14's `text/*` expectations never change.
- **Gate.** P14's tests pass. After P18 they still pass, **and they fail with P17's code reverted**; that proves the rule.
- **Back-out.** Revert (then take DU-6 a in P18).

**P17a — `fix(uploads): accept audio/mp3 as an alias of audio/mpeg`** · Recommended (DU-6 m-b) · new
- **Contents.** One entry in `is_audio_mime_compatible` [C: `crates/openalpaca_core/src/security/sanitizer/mod.rs:268-282`], which both the upload route and connectors consult [C: `files_types.rs:29`, `sanitizer/mod.rs:218`]. It lands before P18, like P17. The commit body names the side effect: an `audio/mp3` declaration with MPEG-1 frames, refused today, is accepted too [inferred].
- **Gate.** P14's `audio/mp3` connector case passes before and after P18, and fails after P18 with P17a reverted.

**P18 — `chore(deps): infer 0.22`** · Required unless DU-6 (c) · replay 15
- **Contents.** `Cargo.toml:144` `infer = "0.22"`. Under DU-6 (a), P14's `text/*` expectations are updated here instead. P14's `heix` connector case changes from OK to BLOCKED here, unless DU-6 (m-c) was taken. So does the `audio/mp3` case, unless P17a landed. The commit body names each case that changes.
- **Gate.** G-check; `cargo test -p openalpacad -- routes::files`; `cargo test -p openalpaca_core -- sanitizer`; `cargo test -p openalpaca_connectors -- common::`.
- **Back-out.** Revert.

#### Phase G: MCP client

**P19 — `chore(deps): rmcp 0.16 → 0.17`** · Required (DU-5) · replay 16
- **Contents.** `crates/openalpaca_mcp/Cargo.toml:26`; lock only. The protocol offered becomes 2025-06-18.

**P20 — `chore(deps): rmcp 0.17 → 1.8, building its non-exhaustive model types through constructors`** · Required (DU-5) · replay 17
- **Contents.** `Implementation::new`, `CallToolRequestParams::new(..).with_arguments(..)`, the `InitializeResult` builder in the test stub, and 18 `ToolAnnotations` literals rewritten by field name. The protocol offered becomes 2025-11-25.

**P21 — `chore(deps): rmcp 1.8 → 2.2, ContentBlock replaces Content/RawContent`** · Required (DU-5) · replay 18
- **Contents.** As replay 18, plus the regenerated `docs/api/crates/openalpaca_mcp.md`: run `python3 scripts/gen_api_docs.py` and commit its output.
  - The re-export line at `:30` changes from `Content, …, RawContent` to `ContentBlock`.
  - Measured: `--check` exits 0 at the base and at `908050f1`, exits 1 on the combined state naming only that file, and exits 0 again after regeneration, with a one-line diff [M: `SP/logs/crit/genapi-{head,combined}.txt`, `SP/logs/rev2-genapi-*.txt`].
  - The commit body lists the **removed public re-exports `openalpaca_mcp::{Content, RawContent}`** and their replacement, `ContentBlock`.

**P22 — `chore(deps): rmcp 2.2 → 3.4.1, server identity is optional`** · Required (DU-5) · replay 19
- **Contents.** The `client.rs` handshake and source citations; the lock brings process-wrap 10.0.1; rustfmt of the layout hunks the rmcp lines added. P22's rustfmt reflows the `lib.rs` re-export. The generator collapses whitespace, so its output should not change [inferred from its single-spaced output line]. The gate re-runs `--check` to confirm.

**Gates for P19–P22.** Pass means 0 failed; the counts are reference points measured on the rmcp worktree.
- Each commit:
  - `cargo check --all-targets` on `-p openalpaca_mcp --features test-utils`, `-p openalpaca_core` and `-p openalpacad`;
  - `cargo test -p openalpaca_mcp --features test-utils`: 34 passed / 2 ignored [M];
  - `cargo test -p openalpacad managers::mcp`: 51 [M].
- P20 also runs `cargo test -p openalpaca_core --lib`: 2023 on the rmcp worktree [M], the permission-tier, sandbox and annotation tests. In sequence the count is higher, because P14, P17 and P17a add core tests. It also runs `-p openalpacad routes::tools` (8 [M]).
- P21 also runs `-p openalpaca_core --lib tools::mcp`, and `python3 scripts/gen_api_docs.py --check`, which is clean after the regeneration.
- P22 also runs:
  - G-CI (including `gen_api_docs.py --check`) and G-lint. `result_large_err` at `openalpaca_mcp/src/client.rs:663` remains.
  - **A manual real-server round trip**, under the §5.2 isolation rule:
    - `cargo test -p openalpaca_mcp -- --ignored`, which runs the npx `@modelcontextprotocol/server-filesystem` e2e and needs the network;
    - one MCP server the owner actually uses, over stdio and over HTTP if they have one, through the daemon: tools list, one call, disable, enable.
  - **Disabled means the child is gone, descendants included (ADR-030).** process-wrap 10.0.0 reworked the stdio child spawner: a wrapper registry, "expose direct process-group children", Windows job objects, and breaking API changes [D: `process-wrap-10.0.1/CHANGELOG.md`]. It was released 2026-08-24; 10.0.1 followed on 2026-09-23.
    - The existing test covers the **direct** child: `a_disable_kills_the_child_refuses_both_arms_and_a_re_enable_restores_cleanly` [C: `apps/openalpacad/src/managers/mcp/tests.rs:407`], whose stub is a single `sh` process. It passed on 3.4.1 with process-wrap 10.0.1 [M: 51/0], and CI's Linux run repeats it.
    - No test covers **descendants**. So, by hand on macOS, with a stdio server that forks (the npx filesystem server: `npx` → `node`), record `ps -o pid,ppid,pgid,command` for the server's tree before the disable. After the disable, none of those pids may remain. Repeat after the re-enable and a second disable.

**Back-out.** Revert the stages in reverse order. The 3.x code does not compile against 2.x [M].

**P23 — `chore(deps): take rmcp without default features`** · Optional (DU-5) · `SP/patches/rmcp-optional-no-default-features.diff` (not replayed on the combined base)
- **Gate.** As P22's per-commit checks, plus `cargo tree -d`.

#### Phase H: web and CLI

**P24 — `test(daemon): pin the CORS headers the GUI relies on`** · Recommended · new
- **Contents.** Extract `fn cors_layer() -> CorsLayer` in `apps/openalpacad/src/router.rs`. A `oneshot` test asserts `*` for allow-origin, allow-headers, allow-methods and expose-headers on a GET with `Origin` and on a preflight, and deliberately does **not** assert `Vary`. The test needs `tower` (`util`) as a dev-dependency.
- **Gate.** Passes on tower-http 0.6.11, and unchanged after P25.

**P25 — `chore(deps): tower-http 0.7`** · Required (DU-11) · replay 20
- **Contents.** `Cargo.toml:64`.
- **Gate.** G-check; `cargo test -p openalpacad` (with P24); the E5 GUI smoke (ETag, If-Match).

**P26 — `chore(deps): tokio-tungstenite 0.30`** · Required (DU-11) · replay 21
- **Contents.** `Cargo.toml:67`.
- **Gate.** Pass means 0 failed.
  - `cargo test -p openalpacad -- events chat graceful_shutdown cors`. This is the filter that was measured: 71 passed [M: web report step 4], of which 41 `routes::chat`, 9 `events::persistence`, 8 `routes::events_history`, 6 `routes::chat_types`, 2 `routes::events`, 2 `graceful_shutdown_tests`, 2 `connector_bridge` and 1 `event_bridge`. P24's CORS test adds to it.
  - `cargo test -p openalpaca`: 249/0/1 at `5e5ac317` [M], and 260/0/1 at `908050f1` with the combined set [M, rev 2].
  - Manual, under the §5.2 isolation rule: `openalpaca daemon tail` and `openalpaca tasks confirmations watch` each print "Connection closed by server" on `openalpaca daemon stop`.

**P27 — `chore(deps): rustyline 18`** · Required · replay 22
- **Contents.** `Cargo.toml:121`, and the doc comment at `apps/openalpaca/src/repl/completer/mod.rs:1` ("rustyline v17" → v18).
- **Gate.** `cargo test -p openalpaca`, then by hand at a real terminal, under the §5.2 isolation rule (from a scratch directory outside any repository):
  - history written by a pre-bump build loads;
  - Tab completes `/mo`, and the hint shows;
  - Ctrl+C mid-stream prints "(interrupted)", and the next prompt works;
  - **Ctrl+C while the first message's POST is in flight** (the skeptic's window);
  - Ctrl+C at an empty prompt continues;
  - resizing the window while typing works;
  - `exit` saves the history;
  - one run with `NO_COLOR=1`.
- **Back-out.** If the REPL dies on the in-flight Ctrl+C, either add a follow-up `fix:` that installs the tokio `ctrl_c` handler once at REPL start (with a test), or revert P27 and hold 17.

**P28 — `chore(connectors): teloxide over rustls with OS roots`** · Optional (DU-12)
- **Contents.** `Cargo.toml:127` `teloxide = { version = "0.17.0", default-features = false, features = ["macros", "ctrlc_handler", "rustls-native-roots"] }`. `README.md:29` drops the `libssl-dev` that P13 added.
- **Gate.**
  - G-check.
  - `cargo test -p openalpaca_connectors --features telegram,discord`: 39 at `5e5ac317` [M]. Re-measured in rev 2 at `908050f1`, on top of the combined set, after the tip's Telegram delivery changes: 56 passed / 0 failed; the lock drops the same 8 crates; `check -p openalpacad --all-targets` gives 0 errors [M: `SP/logs/rev2-tip2-p28-*.log`].
  - `cargo tree -i native-tls --workspace --target all` prints nothing.
  - A live bot start and one message round trip, under the §5.2 isolation rule.

#### Phase I: configuration formats

**P29 — `chore(deps): toml 1.1 and toml_edit 0.25.15`** · Required · replay 23
- **Contents.** `Cargo.toml:114` `toml = "1.1"`; `Cargo.toml:117` `toml_edit = "0.25.15"`. The lock moves edges only; no `--precise` step is needed after P04 [M]. The commit body names the seconds-less time spelling, the `.000` → `.0` spelling, and the newly accepted dotted keys.
- **Gate.**
  - G-check.
  - `cargo test -p openalpaca_llm -p openalpaca_plugins -p openalpaca_core`.
  - `cargo test -p openalpaca config`.
  - `cargo test -p openalpacad -- routes::skills routes::extensions hot_reload services::agents bootstrap`: 74 [M].
  - tauri-utils' toml edge is on 1.1.6.
  - **The E6 owner check of every real TOML file (§6), before this commit (§5.1 item 3).**
- **Back-out.** Revert. [rollback M: 0.9 reads 1.1's output]

**P30 — `chore(deps): replace the archived serde_yaml with serde-saphyr (serde_yaml-compatible options)`** · Recommended (DU-4 C) · replay 24
- **Contents.**
  - `Cargo.toml:43` and `crates/openalpaca_core/Cargo.toml:24`.
  - `middleware/skill/mod.rs` gains `yaml_options()` (`strict_booleans`, `merge_keys: AsOrdinary`, `duplicate_keys: LastWins`, `reject_non_finite_typeless_float: false`, `ignore_binary_tag_for_string`, `with_snippet: false`) and `parse_yaml()`.
  - `renderer.rs`.
  - The pin test `yaml_1_1_spellings_read_the_way_serde_yaml_0_9_read_them` in `skill/tests.rs`.
  - A CLAUDE.md line on the crate and why its options exist.
  - Under DU-4 (B): `SP/patches/config-alt-serde_norway.diff`. Core compiles and its skill tests pass; the rest of the workspace was not re-run for that variant. Under (A): drop the commit.
- **Gate.**
  - `cargo test -p openalpaca_core --lib middleware::skill`: 20 [M]. The pin test fails with the options removed [M].
  - The full core suite: 2024 [M].
  - The daemon's `routes::skills`.
  - **The E6 owner check of their skill directories (§6), before this commit.**
- **Back-out.** Revert. SKILL.md files are only read, never rewritten.

#### Phase J: GUI JavaScript

**P31 — `chore(gui): minor/patch JS bumps and a re-resolved bun.lock`** · Required · replay 25
- **Contents.** The 14 exact pins of §1.3. `bun.lock` is regenerated from scratch. Under DU-13 (b), the recommendation, that is done with Bun 1.3.x, so it stays `"lockfileVersion": 1`, as the researched lock is [M]. Under (a) it is done with 1.4.2 and becomes v2. `CLAUDE.md:152`: React 19.2 → 19.3.
- **Gate.** The four bun gates (97 files / 1165 [M]); `bun audit` reports 0 and exits 0 [M]; `bun outdated` lists only vite and vitest; the CSS hash is unchanged (`index-D_fw04La.css`); `head -3 bun.lock` shows the lockfile version DU-13 chose. All of this was measured with Bun 1.3.13 and a v1 lock. Re-run the four gates with Bun 1.4.2 (CI does) before merging; that is not measured.

**P32 — `chore(gui): Vite 8 and Vitest 5`** · Recommended (DU-14 a) · replay 26
- **Contents.** Two pins and the regenerated lock. `README.md:208`, `CLAUDE.md:152` and `apps/openalpaca-gui/README.md:14` name Vite 8 and Vitest 5.
- **Gate.**
  - The four bun gates; `bun audit` reports 0.
  - `vite` on a spare port; `vite preview` of Chat, Settings and Memory.
  - The Lightning CSS output in WKWebView. This is carried by **E5**, the packaged build with a throwaway identifier and G-ident, rather than a `tauri dev` session.
    - `tauri dev` runs `prepare-sidecar.ts`, which may skip its build and copy a stale daemon (§3.3).
    - The dev app's WebKit storage, which may hold the owner's `oa-project`, is not isolated [NE which directory it uses, §5.2].
    - If `tauri dev` is used anyway, it needs the full §5.2 isolation rule, with no project selected before any chat turn. It judges CSS only, never daemon behaviour.

**P33 — `chore(gui): replace @vitejs/plugin-react-swc with @vitejs/plugin-react 6.1.1`** · Recommended (DU-14) · `SP/patches/gui-all-plus-plugin-react.diff`
- **Contents.** `apps/openalpaca-gui/vite.config.ts:4` and the devDependency swap.
- **Gate.** The four gates; dist hashes identical to P32's [M]; the dev server serves the React-refresh preamble [M].

#### Phase K: advisories and maintenance

**P34 — `ci: advisories workflow (cargo-deny for RustSec, bun audit for bun.lock) and deny.toml`** · **Recommended (DU-21 a)**; Required in rev 1, relabelled because it is a new enforcement point · replay 27, plus the bun job
- **Contents.**
  - `.github/workflows/audit.yml` runs on push to main, on PRs, and weekly on Monday at 06:17 UTC.
  - Job `rustsec`: `cargo-deny-action@3c634983… # v2.1.1`, `check advisories --all-features --workspace --locked`.
  - Job `bun`, new in rev 2: `actions/checkout` and `oven-sh/setup-bun` by the same SHAs as `ci.yml`, `bun install --frozen-lockfile`, `bun audit`. Exceptions only as `--ignore=<CVE>` with a comment giving the reason [M: the flag exists in Bun 1.3.13].
  - `deny.toml`: `yanked = "deny"`, `unmaintained`/`unsound = "workspace"`, and an empty ignore list whose entries must give reasons.
  - `README.md:205` and `CLAUDE.md:154` name the new workflow and who answers it (DU-21).
- **Prerequisites.** DU-21. P04, P07, P15, P16, P22; P31 for the bun job. Under DU-5 (C), add an `ignore` entry for RUSTSEC-2026-0189 with its reason.
- **Gate.**
  - Locally, `cargo deny --locked --all-features --workspace check advisories` reports `advisories ok` [M, combined lock; HEAD fails with 7 errors].
  - `bun audit` exits 0 on P31's lock [M: re-resolved lock], and exits 1 on the pre-wave lock [M].
  - Both jobs are green on the PR. zizmor still reports 0 on `audit.yml`; the bun job was not in the zizmor run, so re-run it.

**P35 — `ci: Dependabot for cargo (lockfile-only), bun and GitHub Actions`** · Recommended (DU-15 a) · replay 28, plus the Tauri `ignore` rules
- **Contents.** As replay 28, plus under DU-15 (p-i) two `ignore` entries:
  - `tauri*` with `update-types: ["version-update:semver-minor"]` in the cargo entry;
  - `@tauri-apps/*` with the same update type in the bun entry.
- **Gate.**
  - GitHub's Dependabot view lists the three ecosystems with no config error. The first run opens at most 3 PRs, and the cargo PR edits only `Cargo.lock`.
  - **The first bun PR keeps `bun.lock`'s `lockfileVersion` as committed** (1 under DU-13 (b)). Under DU-13 (a), expect Dependabot's bun job to **fail** with an unsupported-lockfile error until its bundled Bun is ≥ 1.4 [Docs, §2.10]. Check the Dependabot view for errors, not only for PRs.
  - **No Dependabot PR moves a `tauri*` crate or an `@tauri-apps/*` package to a new minor.** That checks the `ignore`, whose effect under `lockfile-only` is inferred, not verified.
  - The owner enables Dependabot alerts.

#### Phase L: close-out

**P36 — `style: address clippy lints new in Rust 1.94–1.98`** · Optional (DU-17 b/c)
- **Contents.** The 17 sites of §2.2 and nothing older:
  - 13 mechanical fixes;
  - keep `apps/openalpacad/src/event_bridge.rs:37`'s `created_by: _` with an `#[allow]`;
  - fix or `#[allow]` `result_large_err` at `apps/openalpacad/src/routes/extensions.rs:684` and at `crates/openalpaca_mcp/src/client.rs:663` (against rmcp 3.4.1's `ClientInitializeError`);
  - `unnecessary_sort_by` with `Reverse` at `crates/openalpaca_core/src/session_log/record.rs:425`.

  **Never a blanket `clippy --fix`**: it breaks the build (§2.2).
- **Gate.** Clippy unique count 96 → ≤ 80; G-CI.

**P37 — `chore: repository hygiene`** · Recommended (DU-19)
- **Contents.** `git rm rust_out`, plus `/rust_out` in `.gitignore`; `git rm pnpm-workspace.yaml`; `cargo build --locked` and `bun install --frozen-lockfile` in `scripts/release/package-macos.sh:55,60`, `package-linux.sh:48,53` and `package-windows.ps1:58,64`.
- **Gate.** The CI gui job is green. The owner runs `package-macos.sh` once, with `CARGO_TARGET_DIR` unset and the sidecar copy removed first, and the result passes G-ident (§6 E5 gives the sequence).

**P38 — `docs: record the toolchain and dependency wave`** · Required
- **Contents.**
  - CLAUDE.md's *Current work*: one paragraph with the commit range and what moved. The historical "No dependency was upgraded" at `CLAUDE.md:60` stays as history.
  - CLAUDE.md *Gotchas*:
    - under Bun, the Tauri CLI cannot see the JS packages, so `tauri`/`tauri-plugin-*` and `@tauri-apps/*` must be kept on the same major.minor by hand;
    - never `{:?}` a digest; use `sha256_hex`.
  - `tasks/api-fix-plan.md` §0: one *Owner decisions taken* row per DU taken, with its commit.
  - The stale `serde_yaml` notes that the config group lists at `tasks/memory-redesign/03-design.md:213, :723` and `04-implementation-plan.md:77, :146`: bump each document's rev and revision log.
  - Mirror the MSRV and maintenance policy (DU-3, DU-15) as an ADR in the owner's Obsidian KB. That is not a repository commit.
- **Gate.**
  - `git check-ignore -q` on every new `.md` (the `*.md` blanket ignore).
  - `git grep -n '1\.93'` outside `tasks/`, the eval reports and lock files returns nothing.
  - `scripts/eval/memory/reports/*-2026-09-24.json:12` (historical `rustc`) is untouched.
  - `python3 scripts/gen_api_docs.py --check` is clean.

**P39 — `revert: ci: pin runs-on ubuntu-24.04 for the wave`** · Conditional (only if P01b landed) · new
- **Contents.** Reverts P01b, just before the merge. Its own CI run is the deliberate move to the new image: a failure there is the image's, not the bump's.
- **Gate.** CI green on `ubuntu-latest`, or the failure recorded and fixed as its own commit.

**P40 — `ci: remove the temporary Windows check`** · Recommended (paired with P01a) · new
- **Contents.** Removes P01a's job, just before the merge, unless DU-18 (b) makes it permanent.
- **Gate.** E1 recorded the Windows job's last green run on the wave tip before this commit.

**Count: 43 commits** (rev 1: 38; rev 2 adds P01a, P01b, P17a, P39 and P40).

| Status | Commits |
|---|---|
| Required under the recommended answers (26) | P01–P12, P14–P16, P18–P22, P25–P27, P29, P31, P38 |
| Recommended (12) | P01a, P13, P17, P17a, P24, P30, P32, P33, P34 (DU-21), P35, P37, P40 |
| Conditional (2) | P01b, P39: only if the PR runs while `ubuntu-latest` moves |
| Optional (3) | P23, P28, P36 |

A DU-9 (b') `fix:` commit, if it is not folded into P16, would make 44.

The integrator's 28 replayed commits are P01–P12, P15, P16, P18–P22, P25–P27, P29–P32, P34, P35 (P34 without the rev 2 bun job, P35 without the rev 2 `ignore` entries, P21 without the `docs/api` regeneration).

---

## 6. Verification strategy for the wave

**What CI covers, and it is the first time for some of it.**
- The Linux build and tests: the first Linux compile of zbus/keyring 4, and sysinfo 0.39's Linux zombie path. The `managers::mcp` tests' direct-child kill also runs on Linux with process-wrap 10.
- clippy, and `--locked` (DU-22).
- The four bun gates under the pinned Bun.
- Pester on Windows; the temporary Windows `cargo check` (P01a, until P40).
- The advisories jobs, RustSec and `bun audit` (P34, DU-21), on PRs and weekly.

**What CI cannot cover, and how to cover it.**

| Gap | Why CI cannot | How |
|---|---|---|
| macOS at all | CI is ubuntu and Windows only | Everything in this plan was measured on macOS aarch64. Re-run G-CI locally on the final commit (E3). |
| The Tauri crate | CI compiles nothing under `src-tauri` | G-GUI by hand after P04, P08, P12 and at the end. |
| A packaged app, which daemon it bundles, and CSS in WKWebView | No GUI or bundle job | E5 with G-ident. |
| CSS in WebView2 and webkit2gtk | No GUI job; no Windows or Linux desktop here | DU-14 (w-a) accepts the risk, or (w-b) runs E5b. |
| Any Windows Rust build, and the Windows keychain | No Windows Rust job | DU-18 (a): P01a, a temporary `cargo check` job on the PR, removed by P40. The keychain stays unverified. |
| The Linux Secret Service read-back | No session bus on the runner | DU-10: a container with gnome-keyring unlocked, if Linux users with `use_keychain` exist. |
| The macOS keychain read-back | Rule: never the real keychain | DU-10 (a): a disposable macOS user or VM. |
| Real MCP servers, and descendant processes on disable | Network, and third-party servers | P22's manual round trip and `ps` check. |
| The REPL at a terminal | No TTY | P27's checklist. |
| A live Telegram bot | Network and a token | P28's gate, if taken. |
| The owner's real config and skill files | Rule: `~/.openalpaca` is off limits to research | E6. |
| The owner's real partitions and database under the new build | Same rule | E8, on a copy. |
| `docs/api` staleness | CI does not run `gen_api_docs.py --check` | G-CI's local step; E3. |

**The wave's end-to-end gates** (after P38, on the branch, before the merge). All manual runs follow the §5.2 isolation rule.

- **E1:** the PR's CI is fully green, including the P01a Windows check job (DU-18) on the wave tip. Record its run before P40 removes it.
- **E2:** cargo-deny `advisories ok`, cargo-audit 0 vulnerabilities, `bun audit` 0 (exit 0).
- **E3:** locally on macOS:
  - G-CI with 0 failures (apart from the known flake, which must pass on rerun), `python3 scripts/gen_api_docs.py --check` included;
  - G-GUI;
  - G-lint at 96 unique warnings (fewer after P36) and 1456 fmt hunks;
  - `cargo update --dry-run` locks 0;
  - `bun outdated` is empty.
- **E4:** the memory round trip on an isolated store with outbound networking denied (slice 3's gate procedure), through the CLI and the GUI. It exercises SQLite 3.53, the sha2-named partitions, the memory index, toml config loading and the Tauri shell together.
  - CLI verbs run from a scratch directory outside any repository, or with `--global`.
  - The GUI is E5's bundle, with its throwaway identifier.
- **E5:** a packaged build of the wave tip, built so that it cannot pick up a stale daemon. Rev 1 offered `bun run tauri build` as an equal choice. It is not one: `prepare-sidecar.ts` rebuilds only when `./target/release/openalpacad` is newer than the copy in `src-tauri/bin/`. In the main checkout both are 54,894,912 bytes, dated Sep 24 02:04 [M: `ls`], and that copy carries the pre-bump markers `rmcp-0.16.0` and `3.51.1` [M]. So a plain `tauri build` would bundle the pre-bump daemon, and nothing on screen would say so.
  1. In the checkout at the wave tip, with a clean tree and **`CARGO_TARGET_DIR` unset**, because `prepare-sidecar.ts` copies from the hard-coded `./target` [C: `:42-43`]. If slice 4's packaged verification uses the same checkout at the same time (DU-1 B or C), use a separate worktree instead; its `./target` and `src-tauri/bin/` are its own. Then:
     - `rm -f apps/openalpaca-gui/src-tauri/bin/openalpacad-*`, the gitignored copy, which is regenerated;
     - `cargo build --release --locked -p openalpaca -p openalpacad`.
  2. `cd apps/openalpaca-gui && bun install --frozen-lockfile && bunx tauri build --bundles app --config '{"identifier":"com.tangyunpei.openalpaca-gui.wavecheck"}'`.
     - The throwaway identifier should give the app WebKit storage of its own, so the owner's `oa-project` cannot be read. `--config` merging JSON into the config is [D: `tauri-cli-2.11.5/src/build.rs:54-58`]; that WebKit then uses separate storage is inferred (§5.2 item 3), so step 4 still confirms that no project is selected.
     - With the sidecar removed, `prepare-sidecar.ts` runs `cargo build --release -p openalpacad` (a no-op after step 1) and copies it [C: `:45-56`].
     - `scripts/release/package-macos.sh` does steps 1–2 too (`cargo build --release` then `bunx tauri build --bundles app` [C: `package-macos.sh:55-61`]), but with the real identifier. After the sidecar is removed it is acceptable for G-ident, not for isolation.
  3. **G-ident** on `target/release/bundle/macos/openalpaca-gui.app/Contents/MacOS/openalpacad*` against `target/release/openalpacad`: the same `shasum -a 256`, `rmcp-3.4.1` ≥ 1, `rmcp-0.16.0` = 0, `3.53.2` ≥ 1.
  4. Launch from the isolated shell (§5.2): pre-seed `$T/store/config/llm.toml` with credential discovery off, then run `…/openalpaca-gui.app/Contents/MacOS/openalpaca_gui` (confirm the name with `ls`). Never use `open` or Finder: they do not pass the environment. Before any write, confirm the store root on Settings → Connection and in `GET /v1/status`, the `Config base dir:` line in `daemon.log`, and that no project is selected.
  5. The checks:
     - the first-run card;
     - Settings → Models & keys: add a key, which writes `secret_encrypted` with the new code; restart, and the key still loads;
     - Settings → Persona (ETag); save a memory edit (If-Match);
     - Stop daemon, then Start daemon;
     - Chat, Memory and Settings rendered by WKWebView, compared with a pre-wave bundle. Build that the same way, in a separate export of the pre-wave commit, with a second throwaway identifier, and check it with G-ident's pre-wave markers.
  6. Cleanup (§5.2 item 7), including `~/Library` entries named after the throwaway identifier.
- **E5b** (DU-14 w-b only): the same screens in a Windows (WebView2) and a Linux (webkit2gtk) build of the wave tip, compared with pre-wave builds. G-ident applies to each: the daemon's markers are platform-independent crate paths [inferred].
- **E6:** the owner's real data, read-only, printing only equal or different.
  - **Before any tool touches a real file, change it to print only the equality bit and the file's index**, never a value, key or path fragment. Test it first on a synthetic file.
  - Every real TOML file, through toml 0.9.12 and 1.1.6 (`scratch-config/tomlcmp`) and through toml_edit 0.25.13 and 0.25.15 (`verify/tedit-0.25.13`, `verify/tedit-0.25.15`; source `verify/tedit/main.rs`, whose `apply_delta` is a verbatim copy of `render_config_preserving`'s) [M: skeptic, 50 inputs identical]. `tomlcmp` has no toml_edit dependency [M: its Cargo.toml], so both tools are needed. The files:
    - `llm.toml`, `daemon.toml` and `mcp.toml` in `~/.openalpaca/config/`;
    - the same three in the checkout's `config/`, where `llm.toml` is ignored and present [M] and `daemon.toml`/`mcp.toml` are tracked;
    - the same three in any other directory `OPENALPACA_CONFIG_DIR` has pointed at;
    - `~/.openalpaca/plugins/.permissions.toml`, `plugins/.config/*.toml` and each `plugins/*/plugin.toml`;
    - `config/tools/*.toml` and any legacy `config/agents/*.toml`, in both config directories.
  - The real skill directories (`~/.openalpaca/config/skills/*/SKILL.md` and the checkout's `config/skills/`, plus any plugin-contributed skills), through serde_yaml and the chosen successor (`scratch-config/yamlcmp`).
  - The DU-10 count, over every `llm.toml` listed above.
  - Runs before P04 (toml_edit), P12 (DU-10), P29 and P30.
- **E7:** a rollback rehearsal against E5's isolated store, with a **pre-wave build whose identity is checked**.
  1. In a separate export of the pre-wave commit (`git archive <pre-wave> | tar -x -C $T/prewave`), run `cargo build --release --locked -p openalpaca -p openalpacad` with `CARGO_TARGET_DIR=$T/prewave-target`. No bundle is needed, so the sidecar script is not involved.
  2. G-ident, pre-wave markers: `rmcp-0.16.0` ≥ 1 and `3.51.1` ≥ 1 on `$T/prewave-target/release/openalpacad` [M: the markers exist in a pre-bump release build].
  3. Quit E5's app, stop its daemon, then start the pre-wave daemon in the isolated shell: `OPENALPACA_DAEMON_BIN=$T/prewave-target/release/openalpacad`, with the pre-wave CLI's `openalpaca daemon start`, from a scratch directory. Then check:
     - the database opens (schema unchanged; files portable [M]);
     - the key added in E5 decrypts [M] (the provider lists its key as loaded);
     - the config reads.

  This rehearses the rollback the plan relies on for secrets and the database.
- **E8:** a rehearsal on a **copy of the owner's real store**, before the first start of a bumped build on the real store (§5.1 item 4). It is the only check of the owner's existing partitions, snapshots and database under SQLite 3.53.2 and sha2 0.11; the evidence so far is a synthetic 200-row database and known-answer tests.
  1. Stop the real daemon. `cp -Rp ~/.openalpaca "$T/realcopy"`, which includes `state/openalpaca.db` with its `-wal`/`-shm`, the snapshots, the notes and the config. Restart the real daemon if wanted. The copy holds `.master_key` and the encrypted keys, so keep it under `$T` and delete it afterwards.
  2. In the isolated shell, set `OPENALPACA_HOME_STORE=$T/realcopy`, `OPENALPACA_CONFIG_DIR=$T/realcopy/config` and `HOME=$T/home`, and work from a scratch directory outside any repository.
     - **Never start a daemon on the copy.** The boot catch-up of the learner calls the model with the copy's real keys, and writes learned notes into each turn's real project root [C: CLAUDE.md MD-10; turns store their `workspace_root`].
  3. **Baseline first**, with the pre-wave CLI (E7's binaries): `openalpaca memory doctor --global`, with no `--repair`. Save its report under `$T`. With no daemon running on the copy, doctor scans locally and read-only [C: `apps/openalpaca/src/commands/memory.rs:54-58, :699-710`]:
     - the copy's home partitions;
     - every project partition the copied snapshots name, **in place and read-only**, at its real path [C: `memory_files/doctor.rs:127-160`, `Partition::open_readonly`].

     Project partitions cannot be copied elsewhere for this: their key hashes the canonical partition path [C: `memory_files/mod.rs:157-175`]. The baseline matters because a real store may legitimately list orphaned owner keys, for example after a factory reset (CLAUDE.md).
  4. The bumped CLI, same command. Expect the **same report**: no partition newly quarantined, orphaned or unreadable, and no unreadable snapshot. A difference means the new build reads a partition name, revision hash or snapshot differently. Stop there, before any real start.
  5. On the copied database, under 3.53.2, print only results and counts: `PRAGMA integrity_check`, `PRAGMA foreign_key_check`, the FTS5 `integrity-check`, and row counts per table. `SP/probe`'s `read` subcommand, built in `probe/new` against rusqlite 0.40.2, already does the first and the FTS5 check (read in the probe's own source, `SP/probe/probe_main.rs:152-181`); adapt it to take the path and print only those lines. Also count `file_assets` rows in the retryable `error` state, which P15 will re-extract (the worker's own predicate, `background.rs:88-105`).
  6. Reopen the copy with the pre-wave build again: its `doctor --global` report equals step 3's, and the probe's `old` build (SQLite 3.51.1) gives the same integrity result and counts.
  7. `rm -rf "$T"`.

**Load and timing.** Measure on a machine with no L8 running, or accept the MCP-stub flake and the ChatView timeout as load artefacts. Rerun those modules before calling any failure a regression.

**Runner images.** If the PR runs between 2026-10-19 and 2026-11-19, P01b pins `runs-on: ubuntu-24.04`, so an Ubuntu 26.04 image change is not mistaken for the bump. P39 reverts it just before the merge; its own CI run is the deliberate move to the new image.

---

## 7. Files and docs that change

Citations checked at `29b308ca`. None of the files this table cites changed between `29b308ca` and `908050f1` [M: `git diff --name-only`]. Code line numbers are at `5e5ac317`.

| File:line | Today | Commit |
|---|---|---|
| `rust-toolchain.toml:2` | `channel = "1.93.0"` | P03 → `"1.98.1"` |
| `Cargo.toml:32` | `rust-version = "1.93"` | P03 → `"1.98"` |
| `Cargo.toml:43` | `serde_yaml = "0.9"` | P30 |
| `Cargo.toml:64` | `tower-http = { version = "0.6.8", features = ["cors"] }` | P25 |
| `Cargo.toml:67` | `tokio-tungstenite = "0.28.0"` | P26 |
| `Cargo.toml:94` | `sysinfo = "0.38.1"` | P08 |
| `Cargo.toml:97` | `rusqlite = { version = "0.38.0", features = ["bundled"] }` | P06 |
| `Cargo.toml:100` | `base64 = "0.22.1"` | P09 |
| `Cargo.toml:114` | `toml = "0.9"` | P29 |
| `Cargo.toml:117` | `toml_edit = "0.25"` | P29 |
| `Cargo.toml:121` | `rustyline = "17"` | P27 |
| `Cargo.toml:127` | `teloxide = { version = "0.17.0", features = ["macros"] }` | P28 (optional) |
| `Cargo.toml:139` | `aes-gcm = "0.10"` | P11 |
| `Cargo.toml:141` | `sha2 = "0.10"` | P10 |
| `Cargo.toml:143` | `lru = "0.12"` | P07 |
| `Cargo.toml:144` | `infer = "0.16"` | P18 |
| `Cargo.toml:145` | `pdf-extract = "0.8"` | P15 |
| `Cargo.toml:146` | `zip = "2"` | P16 |
| `Cargo.toml:147` | `calamine = "0.26"` | P16 |
| `Cargo.toml:148` | `keyring = { version = "3", features = ["apple-native", "windows-native", "sync-secret-service"] }` | P12 |
| `crates/openalpaca_mcp/Cargo.toml:26` | `rmcp = { version = "0.16", … }` | P19–P22 (P23) |
| `crates/openalpaca_core/Cargo.toml:24` | `serde_yaml.workspace = true` | P30 |
| `Cargo.lock` | — | every Rust commit (P04 onward) |
| `apps/openalpaca-gui/src-tauri/Cargo.lock` | dead, 5146 lines | P02 (deleted) |
| `.github/workflows/ci.yml:18, 45, 69` | `actions/checkout@v4` | P01 |
| `.github/workflows/ci.yml:19-21` | the libdbus comment and `apt-get install libdbus-1-dev pkg-config` | P13 |
| `.github/workflows/ci.yml:22-25` | `dtolnay/rust-toolchain@master`, `toolchain: 1.93.0` | P01 (deleted) |
| `.github/workflows/ci.yml:26` | `Swatinem/rust-cache@v2` | P01 |
| `.github/workflows/ci.yml:31, 33, 35` | cargo build, test, clippy | P01 (`--locked`) |
| `.github/workflows/ci.yml:46` | `oven-sh/setup-bun@v2` | P01 |
| `.github/workflows/ci.yml:73-76` | Pester `-MinimumVersion` | P01 |
| `.github/workflows/ci.yml` (a new `windows-latest` job) | — | P01a adds it, P40 removes it |
| `.github/workflows/ci.yml` `runs-on` | `ubuntu-latest` | P01b → `ubuntu-24.04`, P39 reverts (both conditional) |
| `.github/workflows/audit.yml`, `deny.toml` | new | P34 (the RustSec job and the rev 2 `bun audit` job) |
| `.github/dependabot.yml` | new | P35 (with the Tauri `ignore` entries under DU-15 p-i) |
| `package.json` (root) | no `packageManager` | P01 |
| `apps/openalpaca-gui/package.json:29, :51-52` and the other pins; `bun.lock` | react 19.2.8, vite 7.3.6, vitest 4.1.11, … | P31, P32, P33 |
| `apps/openalpaca-gui/vite.config.ts:4` | `@vitejs/plugin-react-swc` | P33 |
| sha2 sites: `crates/openalpaca_storage/src/content_io.rs:18`, `database/tests.rs:1097`, `store/tests.rs:440`; `crates/openalpaca_core/src/tools/builtins/update_persona/common.rs:8`, `tests/common/eval.rs:363`; `apps/openalpacad/src/hot_reload.rs:43, 244, 279, 317`; `apps/openalpacad/src/managers/mcp.rs:2435` | `format!("{:x}", …)` | P10 |
| `crates/openalpaca_llm/src/keys/key_encryption/{mod,tests}.rs`; `crates/openalpaca_storage/src/content_io.rs` (test) | — | P05, P11 |
| rmcp sites: `crates/openalpaca_mcp/src/{client,error,lib}.rs`, `tests/filesystem_e2e.rs`; `crates/openalpaca_core/src/tools/builtins/mod.rs`, `tools/config/annotations.rs`, `tools/mcp/bridge.rs`, `tools/registry/{capabilities,tests}.rs`, `security/sandbox/tests.rs`, `tests/mcp_integration.rs`; `apps/openalpacad/src/managers/mcp.rs:2315`, `routes/tools/tests.rs` | — | P20–P22 |
| `crates/openalpaca_core/src/middleware/skill/{mod,renderer,tests}.rs` | serde_yaml | P30 |
| `apps/openalpacad/src/extraction.rs` (tests), `routes/files*` tests, `crates/openalpaca_core/src/security/sanitizer` tests | — | P14–P18 |
| `apps/openalpacad/src/routes/files_types.rs:18`, `crates/openalpaca_core/src/security/sanitizer/mod.rs:215` | — | P17 |
| `crates/openalpaca_core/src/security/sanitizer/mod.rs:268-282` (`is_audio_mime_compatible`) | m4a, wav, flac aliases | P17a (`audio/mp3` ↔ `audio/mpeg`) |
| `apps/openalpacad/src/extraction.rs:102` | cells joined as-is | P16 or a sibling `fix:` under DU-9 (b') |
| `docs/api/crates/openalpaca_mcp.md:30` (generated) | re-exports `Content`, `RawContent` | P21 (regenerated, never hand-edited) |
| `apps/openalpacad/src/router.rs:477` (+ a dev-dependency) | `CorsLayer::permissive()` | P24 |
| `apps/openalpaca/src/repl/completer/mod.rs:1` | "for rustyline v17" | P27 |
| `README.md:26` | "The repo pins 1.93.0" | P03 |
| `README.md:29` | Linux: `libdbus-1-dev pkg-config` | P13 (dbus narrowed to the desktop app; `libssl-dev` added), P28 (`libssl-dev` removed, if taken) |
| `README.md:205` | "CI … has three jobs" | P34 |
| `README.md:208` | "Rust 1.93.0 … Vite 7, … Vitest 4" | P03 (Rust), P32 (Vite, Vitest) |
| `CLAUDE.md:152` | "Rust 1.93.0 … React 19.2 … Vite 7, Vitest 4, Bun" | P01 (Bun), P03 (Rust), P31 (React), P32 (Vite, Vitest) |
| `CLAUDE.md:154` | the CI description | P01 (toolchain, `--locked`), P34 (advisories) |
| `CLAUDE.md` *Current work*, *Gotchas* | — | P30 (YAML crate line), P38 |
| `apps/openalpaca-gui/README.md:14` | "Vite 7" | P32 |
| `scripts/release/package-{macos,linux}.sh`, `package-windows.ps1` | no `--locked` / `--frozen-lockfile` | P37 |
| `rust_out`, `pnpm-workspace.yaml`, `.gitignore` | — | P37 |
| `tasks/api-fix-plan.md` §0; `tasks/memory-redesign/03-design.md:213, :723`; `04-implementation-plan.md:77, :146` | — | P38 |
| **Must not change:** `scripts/eval/memory/reports/*-2026-09-24.json:12` | historical `rustc 1.93.0` | — |

---

## 8. Keeping current afterwards

The approach is sized for a repository with one owner.

- **Monthly, by Dependabot** (P35, DU-15), at most three grouped PRs:
  - the Cargo.lock refresh, transitive crates included: this is what P04 did by hand, and what rustls needed;
  - JS minor and patch updates;
  - the actions' SHA pins.

  Each PR must pass the CI gates. A 7-day cooldown keeps a release that is yanked or compromised in its first week out of the PR.
- **Per-PR checklist for every dependency PR, Dependabot's included.** CI alone lets several things merge green.
  1. **The data-path watch-list** (P04). For each listed crate the PR moves, read its changelog before merging: libsqlite3-sys (the bundled SQLite), rusqlite, aes-gcm/aes, sha2, blake3, keyring, serde, serde_json, chrono, unicode-normalization, regex*, toml*, encoding_rs, base64.
  2. **G-GUI by hand** when the PR moves anything the GUI links (tauri*, wry, tao, openalpaca_storage's dependencies). CI compiles nothing under `src-tauri`.
  3. **The format-pinning tests ran and passed** in the PR's CI log: P05's AES fixture and `content_io` known answer, the memory owner-key, partition-key and revision known answers, the schema-42 refusal, the serde-saphyr pin test (P30), and P14's extraction and sniffing characterisations. A pin that fails is a format change, never a test to update in passing.
  4. **The Tauri pairing.** No PR moves `tauri`/`tauri-plugin-*` or `@tauri-apps/*` to a new minor on one side only. Under DU-15 (p-i) Dependabot's `ignore` holds minors back, so a Tauri minor is a hand-made commit that moves both locks together, followed by G-GUI and an E5-style packaged check. Under Bun the Tauri CLI's own check compares nothing [M].
  5. **The lockfile version.** A PR touching `bun.lock` keeps `"lockfileVersion"` as committed (DU-13).
  6. **A packaged build gets G-ident** whenever one is made from a dependency PR.
- **Weekly: the advisories jobs** (P34, DU-21): RustSec through cargo-deny, and `bun audit` for `bun.lock`. Dependabot does no security updates for bun [Docs], so this job is the only thing that notices a JS advisory. A red run on an unchanged lock means a new advisory:
  - fix it with a lock refresh or a bump;
  - or add a reasoned exception: an `ignore` entry in `deny.toml`, or `bun audit --ignore=<CVE>` with a comment.

  Once a year, read cargo-audit's transitive unmaintained list (9 warnings today).
- **Freshness.** A hand-planned bump takes, in both ecosystems, the newest release that is at least 7 days old (§5.1 item 5).
- **Semver-major bumps stay hand-planned,** one commit per crate (per stage for multi-major jumps like rmcp), with its format-pinning test first when the crate touches a data format. This plan is the template.
- **The Rust toolchain.**
  - Move the pin deliberately: every second or third stable release, or sooner when a point release fixes a miscompilation on a target we ship.
  - Always pin an exact x.y.z, and prefer an x.y.1 where one exists.
  - `rust-version` moves with it (DU-3).
  - The procedure is P03's gate: compare the clippy and fmt counts with the previous toolchain, run the full suite and G-GUI, and read the future-incompat list. Re-run the lock refresh after it.
- **Bun and Node.** Bump the `packageManager` pin deliberately, together with the local Bun, installed at exactly that version (§5.1 item 2). Under DU-13 (b), watch dependabot-core's `bun/Dockerfile` (`ARG BUN_VERSION`, 1.3.14 on 2026-09-25). Once it is ≥ 1.4, move `bun.lock` to v2 in its own commit (§4 DU-13). Node stays with the runner image until a floor is threatened.
- **Owner settings, once:** Dependabot alerts on; local Bun at the pinned version.

---

## Appendix A — Evidence index

**The scratch directory is temporary.** If the owner accepts this plan, copy the following somewhere durable before the session's scratch space goes, for example under `tasks/deps-upgrade/`. Its `.md` files then need a `.gitignore` whitelist line (the `*.md` blanket ignore). Copy **sources only, never a `target/` directory**.

| What | Why | Size (sources) [M: `du`] |
|---|---|---|
| `SP/patches/combined-commits/`, `SP/patches/*.diff` | The reference results for every commit | — |
| `SP/reports/` | The evidence behind this plan | — |
| `SP/scratch-config/tomlcmp/`, `SP/scratch-config/yamlcmp/` | E6's toml 0.9 ↔ 1.1 and YAML comparators | 228 K, 728 K |
| `SP/verify/tedit/`, `SP/verify/tedit-0.25.13/`, `SP/verify/tedit-0.25.15/` | E6's toml_edit leg; `tedit/main.rs` holds the verbatim `apply_delta` | 212 K, 20 K, 20 K |
| `SP/tools-tc/lintcount.py`, `lintdiff.py` | G-lint's counters | 8 K |
| `SP/probe/` (`probe_main.rs`, `old/`, `new/`; skip `data*/`) | The SQLite cross-version probe, which E8 adapts | about 12 MB with `old/`/`new/` |
| `SP/scratch-secrets/compat/` | The P11 AES/SHA/base64 differential | 164 K |
| `SP/scratch-docs/make_fixtures.py`, `SP/scratch-docs/cmp/`, and `fixtures/e01_p01_encrypted_v4.pdf`, `fixtures/d02_bzip2_entry.docx` | P14's byte literals and how they were made | small |
| `SP/int/replay.sh`, `c.sh`, `rep.py`, `lockdiff.py`, `dupcount.py`, `fmtfix.py`, `mkdiff.sh` | The replay driver that DU-1 relies on for re-cutting the patches at a new base | 14 K |

**Every tool that will touch a real file prints only an equality bit** (or a count) before it is run on one. This applies to `tomlcmp`, `yamlcmp`, `tedit` and the probe. As written they print values.

**Housekeeping after copying.** Done in rev 2.1: the copy above, and the removal of the nine research worktrees. What remains is the `tasks/todo.md` entry, which belongs to the wave's start.
- Nine research worktrees under `SP` are registered in the owner's repository [M: `git worktree list`]:
  - `wt-combined`, `wt-config`, `wt-deps`, `wt-docs`, `wt-rmcp`;
  - `wt-secrets`, `wt-storage`, `wt-toolchain`, `wt-web`.
- Remove exactly those, with `git -C <repo> worktree remove --force <SP>/wt-<name>` for each. `--force` is needed because they carry uncommitted patch states, which are saved in `SP/patches/`. If the scratch directory is already gone, use `git worktree prune`.
- **Leave the other session's worktrees alone**: `…/43003513-…/scratchpad/memeval/wt-B`, `wt-head` and `wt-l8`, which the L8 evaluation uses.
- Rev 2's own scratch exports (`SP/rev2-tip`, `SP/rev2-tip2`, `SP/rev2-bunaudit`) are plain directories, not worktrees.
- Record the wave in `tasks/todo.md` (§5.1 item 7).

**Reports** (`SP/reports/`):

| Report | Scope |
|---|---|
| `toolchain.md` | Rust 1.98.1, the MSRV, the lock refresh, clippy/fmt/test comparisons |
| `secrets.md` | aes-gcm, sha2, base64, keyring, rand, blake3 |
| `storage.md` | rusqlite/SQLite, lru, sysinfo |
| `documents.md` | pdf-extract, calamine, zip, infer |
| `mcp-client.md` | rmcp |
| `web.md` | tower-http, tokio-tungstenite, rustyline, teloxide TLS, the network duplicates |
| `config.md` | toml, toml_edit, the serde_yaml successor, console, dialoguer |
| `gui-js-bun-tauri.md` | JS packages, Bun, Node, the Tauri pairing |
| `ci-supply-chain.md` | actions, the dead lockfile, advisories, duplicates, version citations, maintenance |
| `verify-data-safety.md` | the skeptic's verdicts and the new risks |
| `integration.md` | the combined state, the order, the 28-commit replay, the branch-tip check |

**Patches** (`SP/patches/`):

| Patch | What it is | Status |
|---|---|---|
| `combined.diff` | The whole set against `5e5ac317` | Compiles; 4169/0/13; applies to `f5199123` (4186/0/13) |
| `combined-commits/01…28-*.diff` | The replayed series | Each Rust commit compiles |
| `toolchain.diff` | Pin + `rust-version` + lock refresh | |
| `toolchain-clippyfix.diff` | The repaired `clippy --fix` | Compiles; 4166/0 |
| `toolchain-clippyfix-raw.diff` | The raw `clippy --fix` | **Does not compile** |
| `secrets.diff` | The secrets group's set | |
| `storage.diff`, `storage-{1-rusqlite,2-lru,3-sysinfo}.diff` | The storage group's set, whole and staged | |
| `documents.diff` | The documents group's set | |
| `rmcp.diff`, `rmcp-stage{1..4}-*.diff`, `rmcp-optional-no-default-features.diff` | rmcp, whole, staged and the default-features option | |
| `web.diff` | The web group's set | |
| `config.diff`, `config-alt-serde_norway.diff` | The config group's set and the DU-4 (B) alternative | |
| `gui-minor-freshlock.diff`, `gui-all.diff`, `gui-all-plus-plugin-react.diff`, `gui-tauri-crates.diff`, `gui-final.diff` | The GUI group's stages | |
| `ci-supply-chain.diff` | The CI group's set | |

**Key logs** (`SP/logs/`):

| Log | What it holds |
|---|---|
| `int-s13-test-full.log` | The combined full suite |
| `int-tip-test-full.log` | The suite on `f5199123` |
| `int-s11-lintcounts.txt` | Combined clippy counts |
| `int-deny-*.log`, `int-audit-combined.log` | Advisories |
| `int-tree-dups-*.log` | Duplicate counts |
| `int-rp-*.log`, `SP/int/replay/summary.tsv` | The replay |
| `tc-AB-lintdiff.txt` | The 17 new lints |
| `tc-E-update.log` | The 80-package refresh |
| `secrets-compat-run.txt` | The AES/SHA/base64 differentials |
| `storage-probe-crossversion.log`, `storage-probe-fp.log`, `storage-probe-newsyntax.log` | SQLite |
| `docs-compare-min.txt`, `docs-audit-*.txt` | Documents |
| `rmcp-*` and `SP/rmcp-research/probe/*.txt` | Protocol, redirect and loop probes |
| `web-cors-probe-th06.txt`, `web-cors-probe-th07.txt` | CORS |
| `SP/scratch-config/probe-diff.txt`, `tomlcmp-results.txt`, `yamlcmp-diff-table.txt` | Config formats |
| `gui-*` | The GUI stages |
| `ci-*` | CI and supply chain |
| `SP/logs/verify/*` | The skeptic's own experiments |

**Rev 2's logs** (`SP/logs/`):

| Log | What it holds |
|---|---|
| `rev2-apply-check-dcc94410.txt`, `rev2-apply-check-908050f1.txt` | `combined.diff` applies to both later tips |
| `rev2-tip2-check.log` | `check --locked` of the combined set at `908050f1`: 0 errors, 1 warning |
| `rev2-tip2-test-cli.log`, `rev2-tip2-test-connectors.log`, `rev2-tip2-test-core-text.log` | 260/0/1, 112/0, 16/0 at `908050f1` |
| `rev2-tip2-p28-update.log`, `rev2-tip2-p28-test.log`, `rev2-tip2-p28-check.log` | P28 at `908050f1`: 8 crates removed, 56/0, 0 errors |
| `rev2-genapi-tip-before.txt`, `rev2-genapi-tip-combined.txt`, `rev2-genapi-regen.txt` | `docs/api` clean at the tip, stale with the set, clean after regeneration |
| `rev2-bun-audit-baseline.txt`, `rev2-bun-audit-fresh.txt` | `bun audit` exits 1 with 10 advisories, and 0 on the re-resolved lock |

**Tools for E6:** `SP/scratch-config/tomlcmp`, `yamlcmp`, and `SP/verify/tedit-0.25.{13,15}` (source `SP/verify/tedit/main.rs`). They print values; change them to print only equal or different before running them on real files.

## Appendix B — Open questions nobody could answer

1. Does keyring 4 read an entry keyring 3 wrote, **measured** on a real (disposable) macOS keychain, a Linux Secret Service and Windows? Established from source only.
2. Does the owner's `llm.toml` contain any `secret_ref`, or `[security] use_keychain = true`? (DU-10.)
3. Do the owner's real TOML config files and user-written SKILL.md files parse identically? (E6.)
4. Does any MCP server the owner uses reject protocol 2025-11-25 instead of negotiating down?
5. Does Ctrl+C during the first message's POST kill the REPL under rustyline 18? (Inferred from source.)
6. Is Vite 8's Lightning CSS output equivalent in WKWebView, WebView2 and webkit2gtk? (Checked in Chromium only. E5 checks WKWebView; DU-14 w-a accepts the other two, or E5b checks them.)
7. Does Bun 1.4 leave a v1 lock at v1 on re-save? (Docs, second-hand. That it writes v2 from scratch is now read in the PR's own text, oven-sh/bun#28792. See #21.)
8. Does the Pester suite pass on 6.2.0? (No PowerShell was available.)
9. Does the whole workspace compile on Windows? Does the daemon build on a clean Ubuntu without OpenSSL headers until P28?
10. Did any of our `EXISTS` queries return wrong rows under SQLite 3.51.1?
11. How often do real uploads hit pdf-extract 0.12's Form-XObject font confusion? And XLS (BIFF) extraction under calamine 0.36 was not measured.
12. Would a DWG declared `image/vnd.dwg`, newly accepted, be rejected by providers as an image part?
13. What is the root cause of the MCP stdio-stub test flake, and does it also flake on CI's ubuntu runner?
14. Are the 1.96.1 and 1.97.1 miscompilation fixes contained in 1.98.x? (Inferred from release branching.)
15. Do the linker-output warnings of 1.97 appear on Linux or Windows? (None on macOS.)
16. Does the dropped `Vary` header affect the packaged app's HTTP cache in any webview? (Reasoned harmless; not observed.)
17. Does the combined set pass the **full** suite at the tip the wave is cut from? At `908050f1` it applies, `check --locked` is clean, and the CLI, connector and `utils::text` suites pass [M, rev 2]. The full suite was not re-run there. §5.1 item 6 (G-base) answers it at branch time.
18. How often do connector attachments arrive as a `heix` HEIC or declared `audio/mp3`? (DU-6; both are now refused on that path [M].)
19. Does the `windows-latest` runner build `aws-lc-sys` without NASM or `AWS_LC_SYS_PREBUILT_NASM`? (P01a answers it.)
20. Does Dependabot's `ignore` with `version-update:semver-minor` hold back a `lockfile-only` cargo update of `tauri*`? (Inferred from docs; P35's first run answers it.) Does Dependabot offer a group spanning cargo and bun?
21. When will Dependabot's bun updater ship Bun ≥ 1.4, so `bun.lock` can move to v2? (1.3.14 on 2026-09-25 [Docs].) Does Bun 1.4.2 keep a v1 lock at v1 when it re-saves it? (Docs, second-hand; P35 and the §8 checklist watch it.)
22. Which WebKit data directory does a `tauri dev` run use: `~/Library/WebKit/openalpaca_gui` or the bundle identifier's? Does WebKit follow an overridden `HOME`? (E5's throwaway identifier makes both moot for E5.)
23. After tauri-build's build script overwrites `<target>/<profile>/openalpacad` with the sidecar copy, does a later `cargo build -p openalpacad` in the same target dir restore a fresh daemon, or does cargo consider it up to date? (G-GUI avoids the question with its own target dir.)
24. Does the MCP stdio disable kill a forking server's descendants, e.g. `npx` → `node`, with process-wrap 10? (P22's `ps` check.) Does a pinned older protocol version (DU-5 d) change rmcp 3.4.1's `MCP-Protocol-Version` header?
