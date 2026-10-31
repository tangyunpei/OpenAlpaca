# Integration: do the nine upgrade slices work together, and in which order?

Integrator report, 2026-09-25. Worktree `SP/wt-combined` (detached at `5e5ac317`), target dir `SP/target-int`.
`SP` = `/private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/4b1ee495-f087-45a9-9fa8-4325993552c8/scratchpad`.

Evidence tags: **[measured]** (I ran it; the command and the log under `SP/logs/` are named), **[our-code]** (file:line),
**[dep-source]**, **[docs]**, and **[group]** (a result another group measured and I did not re-run). Anything not
established is said to be not established.

## 0. Verdict

1. **All nine slices combine with no semantic conflict.** On the combined state [measured]:
   - `cargo +1.98.1 check --workspace --all-targets --exclude openalpaca_gui`: 0 errors. The only warning is the known `proc-macro-error2` future-incompat note.
   - **Full test suite: 4169 passed, 0 failed, 13 ignored** (24 test binaries, 278 s; `int-s13-test-full.log`).
   - Clippy: **96** unique warnings, against 97 at HEAD on 1.98.1. No dependency bump adds a lint (§5.2).
   - rustfmt: neutral against HEAD (1456 hunks in 290 files at both) after 8 layout fixes to lines the patches add (§5.3).
   - The GUI crate compiles, and clippy finds 0 warnings in it.
   - The four JS gates pass: 97 files, 1165 tests.
   - The proposed `cargo-deny` gate passes (HEAD fails it with 7 errors), and cargo-audit reports 0 vulnerabilities.
   - `cargo update --dry-run` locks 0 packages, and **every direct dependency is at its latest release**.
2. **The order that works was proven commit by commit.** I restored the worktree to HEAD and replayed the sequence as 28 commits (§3).
   - Every Rust commit passes the workspace check, plus targeted tests where the commit touches data formats.
   - The replay's end state is **byte-identical to the combined state, `Cargo.lock` included**, so the order is reproducible.
   - The 28 patches are in `SP/patches/combined-commits/`, and they apply in sequence to HEAD [measured].
3. **The combined patch also holds on the current branch tip.** `feat/markdown-memory` moved 8 commits past `5e5ac317` (the other session's approval-surface escaping work).
   - `SP/patches/combined.diff` applies cleanly to an export of `f5199123` and compiles `--locked` there [measured].
   - The full suite passes there too: **4186 / 0 / 13** [measured] (§5.8).
4. **No slice needs rework.** The interactions found (§4) are:
   - textual collisions in shared lines (root `Cargo.toml`, the lock files, `ci.yml`, docs), resolved by editing by hand and regenerating the lock;
   - one invalidated verification step: documents' "no quick-xml duplicate" check, because the lock refresh moved `plist` to quick-xml 0.42;
   - two positive interactions: darling 0.23 disappears, and web's RustCrypto 0.11 cost shrinks to `sha1` alone;
   - one trap that disappears if the lock refresh lands first: config's toml `--precise` ordering.
5. **What is still not measured:**
   - any Linux or Windows compile (the ubuntu CI job is the first Linux build of zbus/keyring 4 and sysinfo 0.39);
   - live connectors, the TTY REPL, real MCP servers, the Tauri app in WKWebView;
   - a keychain written by keyring 3 and read by keyring 4, and the owner's real config files.

   All of these carry over unchanged from the group reports (§8).

## 1. What I ran

Every cargo call went through `SP/int/c.sh`, which runs:

```
nice -n 10 env CARGO_TERM_COLOR=never CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=SP/target-int \
  HOME=SP/testhome-int CARGO_HOME=~/.cargo RUSTUP_HOME=~/.rustup cargo +<toolchain> …
```

It writes to `SP/logs/int-<name>.log`, one command at a time.
- The sandboxed `HOME` keeps every test away from `~/.openalpaca`.
- The real keychain is reached only through an `llm.toml` carrying `use_keychain = true` or `secret_ref` keys (`apps/openalpacad/src/services/llm.rs:10-83`, `apps/openalpaca/src/commands/ai_config_helpers.rs:25-42`) [our-code]. This worktree has no `config/llm.toml`, and the sandboxed `HOME` hides the owner's store, so no test could reach it.

Root-manifest edits were made with `SP/int/rep.py`, which fails unless each anchor line matches exactly once. Code patches went through `git apply --exclude=Cargo.lock [--exclude=Cargo.toml]`, and cargo regenerated the lock at every step.

| Step | What | Result [measured] | Log |
|---|---|---|---|
| 0 | HEAD duplicate census (`tree -d -e normal`, host and `--target all`) | 33 names / 71 versions (host); 65 / 151 (all) | `int-tree-dups-head*.log` |
| s0 | `ci-supply-chain.diff` (whole) | applies; 3 YAML files parse (Ruby `YAML.load_file`) | — |
| s1 | `toolchain.diff` (pin 1.98.1, `rust-version` 1.98, lock refresh) | `metadata --locked` ok; check 0 errors, 60 s | `int-s1-check.log` |
| s2 | secrets group's proposed commit 1: the aes-gcm 0.10 fixture test (its text), plus a `content_io::sha256_hex` known-answer test I wrote from its spec | **both pass before the crypto bumps**: key_encryption 14/14, content_io 1/1 | `int-s2-kat-test.log` |
| s3 | storage: rusqlite 0.40.2, lru 0.18, sysinfo 0.39.6 (manifest by hand) | check 0 errors; lock delta identical to the storage group's (9 names, 737→735) | `int-s3-storage-check.log` |
| s4 | secrets: base64 0.23, sha2 0.11, aes-gcm 0.11, keyring 4.2 (code via `git apply`) | check 0 errors; lock delta identical to the group's (removed dbus-secret-service, security-framework 2, core-foundation 0.9, rand_core 0.6, opaque-debug). `test -p openalpaca_llm -p openalpaca_storage`: **246 + 679 passed, 0 failed** (fixture, content_io, owner-key, revision, partition-key, HOME_README digest and schema-42 tests all green) | `int-s4-secrets-*.log` |
| s5 | documents: pdf-extract 0.12, zip 8 (`deflate` only), calamine 0.36, infer 0.22 | check 0 errors, 787→783 packages | `int-s5-docs-check.log` |
| s6 | rmcp 3.4.1 (`rmcp.diff` code) | check 0 errors; darling 0.23 **leaves** the lock | `int-s6-rmcp-check.log` |
| s7 | web: tower-http 0.7.1, tokio-tungstenite 0.30, rustyline 18 | check 0 errors; the only RustCrypto crate added is `sha1 0.11` | `int-s7-web-check.log` |
| s8 | config: toml 1.1, toml_edit 0.25.15, serde-saphyr (code via `git apply`) | check 0 errors; `tauri-utils` stays on toml 1.1.6 with no `--precise` step | `int-s8-config-check.log` |
| s9 | GUI JS `gui-all.diff` | `bun install --frozen-lockfile` (Bun 1.3.13, with the root `packageManager` field present): ok, lock untouched; check / format:check / build / test: all exit 0, **97 files / 1165 tests** | `int-s9-*.log` |
| s10 | `cargo update --dry-run [--verbose]` | "Locking 0 packages"; 5 unchanged behind latest, all held by upstream requirements (generic-array, matchit, and toml 0.8 / toml_datetime 0.6 / toml_edit 0.20 from the GTK build) | `int-s10-*.log` |
| s11 | `clippy --workspace --all-targets --exclude openalpaca_gui --message-format=json` | 140 raw / **96 unique**, 25 lint codes, 0 rustc warnings | `int-s11-clippy.log`, `int-s11-lintcounts.txt` |
| s12 | `fmt --all -- --check` | 1464 hunks before the fix → **1456** after (identical per-file counts to HEAD) | `int-s12-fmt*.log` |
| s13 | `test --workspace --exclude openalpaca_gui --no-fail-fast` | **4169 / 0 / 13** | `int-s13-test-full.log` |
| s14 | `clippy -p openalpaca_gui --all-targets` (empty gitignored sidecar placeholder) | 421 artefacts incl. tauri 2.11.6, wry 0.55.1, `openalpaca_gui`; 0 warnings in the GUI crate; 2 pre-existing `let_and_return` in storage | `int-s14-gui-clippy.log` |
| — | duplicate census, combined vs HEAD (HEAD per binary measured on a `git archive` export of `5e5ac317`) | §5.5 | `int-tree-dups-*.log` |
| — | `cargo-deny 0.20.2 --frozen --all-features --workspace check advisories` with the proposed `deny.toml` (local DB path) | combined: **`advisories ok`**, exit 0; HEAD export: exit 1, **7 errors** | `int-deny-*.log` |
| — | `cargo-audit 0.22.2 --no-fetch` (DB `e2111519`) | combined: **0 vulnerabilities**, 9 allowed warnings (all transitive) | `int-audit-combined.log` |
| — | Linux graph (`tree --target x86_64-unknown-linux-gnu -i …`) | `libdbus-sys` gone outside the GUI; `openssl-sys` still via teloxide native-tls; zbus via keyring 4 | `int-tree-linux-*.log` |
| s15 | `cargo +1.93.0 check` on the combined state | refused: every member "requires rustc 1.98", `sysinfo@0.39.6 requires rustc 1.95` | `int-s15-check-193.log` |
| replay | `SP/int/replay.sh`: restore to HEAD, apply 28 commits, check (+ targeted tests) after each | all green; end state byte-identical to the combined state (§3) | `int-rp-*.log`, `SP/int/replay/summary.tsv` |
| tip | `combined.diff` on a `git archive f5199123` export: `check --locked`, full test suite | §5.8 | `int-tip-*.log` |

Process notes, stated plainly:
- **I made one mistaken command and recovered from it.** A line meant as a no-op ran `git restore --source=HEAD -- Cargo.lock` in the worktree, after the full test run.
  - I restored the combined lock from the step-8 copy (`SP/int/Cargo.lock.s8`), and nothing changes the lock after step 8.
  - `cargo metadata --locked` and `cargo check --locked` then passed in 6 s with nothing recompiled, which proves the restored lock is the one the tests ran against [measured].
  - The replay later reproduced the same lock byte for byte.
- The first replay start failed with "permission denied": the script had no execute bit. Nothing had been applied; the worktree was verified clean and the script was rerun.
- The run's MCP hang tests left two orphaned `sleep 3600` processes (ppid 1). They started at 15:37:44 and 15:37:54, inside my test window; there were 0 before it. I killed those two pids and nothing else.

## 2. The order that works

The binding constraints, each measured unless marked:

| # | Constraint | Why | Evidence |
|---|---|---|---|
| O1 | CI's `ci.yml` change (drop `dtolnay/rust-toolchain` + `toolchain: 1.93.0`) **before or with** the toolchain commit; the toolchain commit then needs no `ci.yml` edit | The toolchain group's commit edits `ci.yml:24`, and the CI group's C1 deletes that line. Doing both is a textual conflict | [measured] the two patches; replay commits 01 and 03 |
| O2 | Toolchain pin + `rust-version = "1.98"` **before** sysinfo 0.39.6 and before the lock refresh | sysinfo 0.39.6 declares 1.95, and the combined state is refused on 1.93.0. Resolving with `rust-version` 1.93 holds takecell at 0.1.1 | [measured] `int-s15-check-193.log`; [group] toolchain E2 |
| O3 | The semver-compatible lock refresh (**commit 04**) **before any semver-major commit** | Each major's lock diff then contains only its own crates (1–46 packages per commit, §3) | [measured] replay |
| O3a | The refresh first also means the toml `--precise` workaround is not needed | `tauri-utils` is already on toml 1.1.6 when the toml commit arrives. Only if toml lands *before* the refresh must the config group's order be followed (`cargo update -p toml@1.1.4 --precise 1.1.6` first) | [measured] tauri-utils edge before and after s8 and in the replay (`SP/int/replay/tauri-utils-toml-edge.txt`) |
| O4 | The pinning tests (commit 05) **before** sha2 0.11 and aes-gcm 0.11 | They must pass on the old code and again after each bump | [measured] s2 (pre-bump), replay 05/10/11 |
| O5 | calamine 0.36 and zip 8 in **one** commit | calamine 0.36 requires `zip ^8.6` | [dep-source, group]; replay 14 |
| O6 | rmcp stages strictly in order 0.17 → 1.8 → 2.2 → 3.4.1 | The 3.x code does not compile against 2.x or earlier | [group]; replay 16–19 each green on the combined base |
| O7 | The advisories gate (`deny.toml` + `audit.yml`) **after** rmcp, pdf-extract, calamine, lru and the lock refresh (rustls) | Without them, `cargo deny` fails with 7 errors | [measured] `int-deny-headexport.log` vs `int-deny-wt-combined.log` |
| O8 | Dependabot after the SHA-pinned CI commit | It maintains those pins | [group] |
| O9 | GUI minor/patch commit, then Vite 8 / Vitest 5; **decide the Bun version before regenerating `bun.lock`** | Bun 1.4 writes lockfile v2, which Bun ≤ 1.3 cannot read | [group]; replay 25/26 |
| O10 | The Tauri crates 2.11.6 / dialog 2.7.3 / fs 2.5.2 need no commit of their own: the lock refresh (commit 04) already carries them | — | [measured] lockdiff HEAD→s1 |
| O11 | Dropping `libdbus-1-dev` from CI only after keyring 4 **and** after C1 | Both edit `ci.yml:19-21`. Only a CI run proves it | [measured] Linux tree has no libdbus-sys outside the GUI |
| O12 | One docs commit, last | `CLAUDE.md:152`, `README.md:26/208` and `apps/openalpaca-gui/README.md:14` are each edited by 2–3 groups | [our-code] |
| O13 | The optional clippy-fix commit after aes-gcm and rmcp | The full reference patch collides with aes-gcm at `key_encryption/tests.rs:7` (§4.1). The `result_large_err` at `openalpaca_mcp/src/client.rs:663` survives rmcp 3.4.1 and must be fixed against 3.4.1's `ClientInitializeError` | [measured] `git apply --check` on the combined state; clippy diff |
| O14 | Land the toolchain commit after the running L8 memory evaluation, or record the rustc change | L8's reports record `rustc` | [group] toolchain §8 |

Within a group the order is free, apart from O5/O6. The storage commits are order-independent when the lock is regenerated; the group's stacked lock hunks assume rusqlite before lru.

## 3. The replayed commit sequence (the plan's commit list)

Restored `wt-combined` to HEAD (`git restore -- .`, the three new untracked files removed), then `SP/int/replay.sh`.
- Every Rust commit ran `cargo +1.98.1 check --workspace --all-targets --exclude openalpaca_gui` (log `int-rp-NN-check.log`), plus the listed tests (`int-rp-NN-test.log`).
- "Locked" is cargo's own "Locking N packages" count at that commit, i.e. how many lock entries the commit adds or moves.

| # | Commit | Check | Locked | Tests [measured] |
|---|---|---|---|---|
| 01 | `ci: pin actions by SHA; take the toolchain from rust-toolchain.toml; packageManager bun@1.4.2` (ci.yml + root package.json) | no Rust | — | — |
| 02 | `chore: remove the src-tauri Cargo.lock cargo never reads` | no Rust | — | — |
| 03 | `build: pin Rust 1.98.1; workspace rust-version 1.98` | 0 errors | 0 (`metadata --locked` ok) | — |
| 04 | `chore(deps): refresh Cargo.lock within semver` (the toolchain group's exact lock, 80 packages; brings rustls 0.23.45, tauri 2.11.6, toml 1.1.6, base64 0.23.1, darling 0.24) | 0 | (lock applied) | — |
| 05 | `test(llm,storage): pin the on-disk secret and hash formats` | 0 | 0 | key_encryption + content_io + memory_files: 107 passed |
| 06 | `chore(deps): rusqlite 0.40.2 (bundled SQLite 3.53.2)` | 0 | 3 | storage `database` + `memory_index`: 104 passed, 3 ignored (benches) |
| 07 | `chore(deps): lru 0.18` | 0 | 1 | — |
| 08 | `chore(deps): sysinfo 0.39.6` | 0 | 2 | `daemon_lifecycle`: 9 passed (incl. the real unreaped-child test) |
| 09 | `build(deps): base64 0.23` | 0 | 0 (already in the lock via hyper-util; only edges move) | — |
| 10 | `build(deps): sha2 0.11` + the 10 `{:x}` rewrites | 0 | 6 | 107 passed (fixture, sha256 known answer, owner-key, revision, partition keys) |
| 11 | `build(deps): aes-gcm 0.11 (aead 0.6)` | 0 | 12 | key_encryption: 14 passed (incl. the 0.10 fixture) |
| 12 | `build(deps): keyring 4.2 (v1)` | 0 | 46 | — (not measurable on a keychain here) |
| 13 | `fix(deps): pdf-extract 0.12 / lopdf 0.42 (RUSTSEC-2026-0187)` | 0 | 11 | — (the documents group's characterisation tests do not exist yet) |
| 14 | `chore(deps): calamine 0.36 + zip 8 with deflate only (RUSTSEC-2026-0194/0195)` | 0 | 7 | — |
| 15 | `chore(deps): infer 0.22` | 0 | 2 | — |
| 16 | `chore(deps): rmcp 0.16 → 0.17` | 0 | 2 | — |
| 17 | `chore(deps): rmcp 0.17 → 1.8` | 0 | 2 | — |
| 18 | `chore(deps): rmcp 1.8 → 2.2 (ContentBlock)` | 0 | 2 | — |
| 19 | `chore(deps): rmcp 2.2 → 3.4.1` + rustfmt of the lines rmcp added | 0 | 3 | `openalpaca_mcp --features test-utils`: 34 passed, 2 ignored (npx e2e) |
| 20 | `chore(deps): tower-http 0.7` | 0 | 1 | — |
| 21 | `chore(deps): tokio-tungstenite 0.30` | 0 | 3 | — |
| 22 | `chore(deps): rustyline 18` | 0 | 3 | — |
| 23 | `chore(deps): toml 1.1 and toml_edit 0.25.15` | 0 | 0 (both already in the lock; edges move) | — |
| 24 | `chore(deps): replace serde_yaml with serde-saphyr (compat options)` + the pin test | 0 | 6 | `middleware::skill`: 20 passed |
| 25 | `chore(gui): minor/patch JS bumps and a re-resolved bun.lock` (`gui-minor-freshlock.diff`) | no Rust | — | [group] four gates |
| 26 | `chore(gui): Vite 8 and Vitest 5` (`gui-all.diff`) | no Rust | — | [measured] four gates on the end state (s9) |
| 27 | `ci: RustSec advisories workflow (cargo-deny) and deny.toml` | no Rust | — | [measured] deny gate passes on this lock |
| 28 | `ci: Dependabot for cargo (lockfile-only), bun and GitHub Actions` | no Rust | — | — |

**Reproducibility [measured]:**
- `diff -r` of the replay's final snapshot against the combined state's snapshot is empty for all 36 present files; the deleted dead lock is absent in both.
- `cmp` of the two `Cargo.lock` files: identical.
- The 28 patches cut from the snapshots (`SP/patches/combined-commits/NN-*.diff`, git format) apply in order to HEAD's versions of the 37 files, and rebuild the combined state byte for byte.

**Not in the sequence** (it does not exist yet, or needs an owner decision or a CI run):
- The documents group's characterisation tests, which precede commit 13.
- web's optional CORS test, which precedes commit 20.
- The teloxide rustls flag (owner decision).
- rmcp without default features (owner decision).
- The `@vitejs/plugin-react` swap (owner decision).
- The libdbus CI follow-up (needs a CI run).
- The docs commit.
- The optional clippy commit.
- The alternative serde_norway patch (owner option B).

## 4. Interactions found

### 4.1 Textual conflicts (resolved; none needs judgement)

| Where | Who | What happens | Resolution |
|---|---|---|---|
| Root `Cargo.toml:91-148` | storage (sysinfo, rusqlite, lru), secrets (base64, aes-gcm, sha2, keyring), documents (infer, pdf-extract, zip, calamine), web, config | The groups edit adjacent lines, so their hunks' context lines are each other's edits. The diffs do not stack with `git apply` | Each commit edits its own requirement lines by hand. Mechanical [measured] |
| `Cargo.lock` in every group patch | all | Every lock hunk was generated against HEAD's lock, so none stacks after the first | Take the toolchain group's lock (commit 04) as-is, then let cargo regenerate after each manifest edit. **Every version any group introduced is in the combined lock, and the combined lock contains no version outside the union of the groups' locks** [measured, `lockdiff`] |
| `.github/workflows/ci.yml:22-25` | toolchain (edit `toolchain: 1.98.1`) vs ci-hygiene (delete the step) | Conflict | O1: take the CI version |
| `.github/workflows/ci.yml:19-21` | secrets' libdbus follow-up vs ci-hygiene | Sequential edits of one block; compatible | O11 |
| Root `package.json` | GUI commit 0 and ci-hygiene C1 both add `"packageManager": "bun@1.4.2"` | The same edit | Land once, in commit 01 |
| `crates/openalpaca_llm/src/keys/key_encryption/tests.rs:7` | toolchain's full clippy-fix reference patch vs secrets' aes-gcm migration | Both rewrite `Key::from_slice(&key_bytes).clone()`. The aes-gcm form (`Key::from(key_bytes)`) already removes the `clone_on_copy` warning | The recommended 17-site clippy commit does not touch it; the rest of the reference patch applies cleanly on the combined state [measured] |
| `apps/openalpacad/src/managers/mcp.rs` | secrets (sha2 at :2432) and rmcp (`Implementation::new` at :2312) | Same file, separate hunks | Apply in either order [measured] |
| `CLAUDE.md:152`, `README.md:26,29,205,208`, `apps/openalpaca-gui/README.md:14` | toolchain, secrets, gui, ci-hygiene | Several groups edit the same lines | One docs commit at the end (O12). Not in my diff |

### 4.2 Resolution interactions (lock and graph)

- **quick-xml does not collapse** (a documents claim invalidated by the order).
  - The documents group expected calamine 0.36 to leave one quick-xml (0.41). In combination, the lock refresh first moves `plist` 1.10.0 → 1.10.1, which requires quick-xml 0.42. The combined lock therefore holds **0.41.0 (calamine) and 0.42.0 (plist, GUI only)** [measured, `int-tree-dups-comb.log`].
  - Not a security issue: 0.41 carries the RUSTSEC-2026-0194/0195 fixes, and cargo-audit is clean.
  - The documents commit's verification line "`cargo tree -d` has no quick-xml duplicate" must be dropped or restated as "no quick-xml ≤ 0.40".
- **darling 0.23 disappears** (positive).
  - rmcp 3.4.1 moves to darling 0.24, which serde_with 3.23 (from the lock refresh) already uses, so rmcp adds no darling version; the rmcp group expected a new darling 0.24 duplicate.
  - Net result: darling 0.20 (tokio-cron-scheduler) + 0.24 [measured, s6 lockdiff].
- **RustCrypto 0.11 arrives once** (positive).
  - secrets (sha2/aes-gcm 0.11) brings digest 0.11, crypto-common 0.2, hybrid-array, block-buffer 0.12 and const-oid.
  - web's tungstenite 0.30 then adds only `sha1 0.11` [measured, s7 lockdiff], as the web group predicted conditionally.
- **base64 0.23 is free.** The lock refresh (hyper-util 0.1.21) and rmcp 3.4.1 both bring base64 0.23.1 regardless, so the secrets group's base64 commit adds no package; only edges move (replay 09).
  - The GUI binary links three base64 versions: 0.21.7 (swift-rs), 0.22.1 and 0.23.1 [measured].
- **process-wrap** moves twice: 9.1.0 → 9.1.1 (lock refresh), then → 10.0.1 (rmcp). The refresh's move is superseded; nothing to do.
- **toml** (O3a). After the toml commit, toml 0.9.12 stays in the lock for `tauri-build → cargo_toml` (a GUI build-dependency). On the host graph, toml, toml_datetime and winnow each drop to a single version [measured].
- **zip's `deflate` feature switches `flate2` to the zlib-rs backend** for every flate2 user in the daemon, as the documents group found. In combination nothing else in the daemon uses flate2 differently; the full suite passes [measured].

### 4.3 Things that did *not* interact (checked)

- **keyring 4**, with `default-features = false, features = ["v1"]`, resolves the same way next to every other slice (the secrets group's 65 introduced packages are all present) [measured].
- **rusqlite 0.40's hashlink 0.12** and **lru 0.18** agree on hashbrown 0.17. hashbrown 0.15, foldhash 0.1 and allocator-api2 leave [measured].
- **sysinfo 0.39.6 and rustyline 18** both build under the same 1.98 resolution. rustyline removes nix 0.30.1 [measured].
- **serde-saphyr and toml 1.1** share nothing.
- **The combined state adds no clippy lint** (§5.2).
- **The per-group targeted tests pass on the combined base in the replay**: storage, crypto known-answer tests, daemon_lifecycle, rmcp, and skill YAML.

## 5. The combined state

### 5.1 Size

`SP/patches/combined.diff`: 37 files, 11,196 lines [measured]. By category:
- Rust source: **+267 / −207** in 24 files.
- Manifests: +27 / −21.
- CI and JS config: +142 / −31.
- Lock files: +1424 / −6063. This includes deleting the 5146-line dead `src-tauri/Cargo.lock`.

The Rust source changes break down as:
- rmcp: 14 files (the constructors, `ContentBlock`, the optional `server_info`);
- sha2: the 10 `{:x}` rewrites in 7 files;
- aes-gcm: `key_encryption`;
- serde-saphyr: `middleware/skill` plus its pin test;
- the two format-pinning tests.

### 5.2 Clippy [measured]

`lintdiff` of `tc-B-clippy198.json` (HEAD on 1.98.1, from the toolchain group) against `int-s11-clippy.log`:
- **−1** `clone_on_copy` at `key_encryption/tests.rs:10` (removed by the aes-gcm migration).
- `useless_vec` ×33 in `tools/registry/tests.rs` only moved lines, because the rmcp builders shortened the file.
- Every other code and location is unchanged: 24 codes.
- `result_large_err` at `openalpaca_mcp/src/client.rs:663` is still there under rmcp 3.4.1 (O13).
- Totals: **97 → 96 unique**, 141 → 140 raw, 0 rustc warnings. CI does not gate on warnings.

### 5.3 rustfmt [measured]

Before my fix, the combined state had 1464 hunks against HEAD's 1456: eight new hunks, all layout-only, all in lines the patches add:
- rmcp: the `from_raw(..)` comment alignment in `tools/config/annotations.rs` and `tools/registry/capabilities.rs`, three one-line `ToolAnnotations::new()…` chains in `tools/registry/tests.rs`, and the `openalpaca_mcp/src/lib.rs` re-export wrap;
- config: an `assert_eq!` wrap in the pin test;
- the fixture test I added: a literal moved to its own line.

I applied exactly rustfmt's suggestion for those eight (`SP/int/fmtfix.py`) and nothing else. The per-file hunk counts then equal HEAD's in all 290 files.
- This is mechanical, and it sits in the same commits (19, 24, 05).
- The tree as a whole remains not rustfmt-clean, as at HEAD, and CI has no fmt gate.

### 5.4 Tests [measured, `int-s13-test-full.log`]

| Binary | passed / failed / ignored |
|---|---|
| openalpaca (CLI) | 249 / 0 / 1 |
| openalpaca_api | 13 / 0 / 0 |
| openalpaca_connectors | 95 / 0 / 0 |
| openalpaca_core (lib) | 2024 / 0 / 5 |
| core `mcp_integration` / `memory_eval` / `memory_eval_corpus` / `steer_routing_eval` | 0/0/1 · 0/0/0 · 14/0/0 · 1/0/1 |
| openalpaca_llm | 341 / 0 / 0 |
| openalpaca_mcp / `filesystem_e2e` | 34 / 0 / 0 · 0 / 0 / 2 |
| openalpaca_plugins | 117 / 0 / 0 |
| openalpaca_storage | 679 / 0 / 3 |
| openalpaca_wake | 17 / 0 / 0 |
| openalpacad / `daemon_args` | 582 / 0 / 0 · 3 / 0 / 0 |
| **Total** | **4169 / 0 / 13** |

- **No failing test.** The known flaky `managers::mcp::tests` stub tests did not fail on this run. The toolchain group showed the flake predates the bump (3/6 passes on 1.93.0 at HEAD); rerun that module on a failure.
- Against the toolchain group's 4166 (its clippy-fix run, 0 failures), the three extra tests are the two format pins (s2) and serde-saphyr's YAML pin.
- The 13 ignored are the storage benches, the live evals, and the npx and MCP e2e tests, all ignored by design.

### 5.5 Duplicates (`cargo tree -d -e normal`, names with ≥ 2 distinct versions) [measured]

| Scope | HEAD | Combined |
|---|---|---|
| Workspace, host | 33 names / 71 versions | 38 / 80 |
| Workspace, all targets | 65 / 151 | 71 / 160 |
| `openalpacad` | 13 | **19** |
| `openalpaca` (CLI) | 9 | **2** (hashbrown 0.14/0.17, syn 2/3) |
| `openalpaca_gui` | 10 | **15** |

The daemon's six extra names:
- aes 0.8/0.9, cipher, crypto-common, digest, inout, block-buffer, cpufeatures and sha2 0.10/0.11: the RustCrypto generation split. lopdf 0.42 and axum 0.8.9's sha1 stay on 0.10 while our aes-gcm/sha2 are on 0.11.
- tower-http 0.6/0.7 (reqwest holds 0.6).
- base64 0.22/0.23.

Removed from the daemon: constant_time_eq, foldhash, toml_datetime, winnow, one hashbrown.

This matches the CI group's all-latest prediction (CLI 2, GUI 15), except the daemon (they predicted 20). The web group's two optional avoidance picks — hold tower-http at 0.6, pin tokio-tungstenite 0.29 — are owner choices. The tungstenite pair never meets in one binary.

### 5.6 Advisories [measured]

- The proposed gate (`deny.toml`: `yanked = "deny"`, `unmaintained` and `unsound = "workspace"`, empty ignore list):
  - **passes** on the combined lock;
  - fails on HEAD with 7 errors: lopdf, 2× quick-xml, rmcp, rustls, 2× lru-unsound.
- cargo-audit on the combined lock: **0 vulnerabilities**; 9 allowed warnings: proc-macro-error, proc-macro-error2, ttf-parser, unic-* ×5, and glib (unsound), all transitive through tauri, teloxide or lopdf.

### 5.7 Lock freshness and latest versions [measured]

- `cargo update --dry-run` locks 0 packages.
- Every row of `SP/direct-deps.tsv` has its `latest` version in the combined lock, except `serde_yaml`, which is replaced.
- Still behind, held by upstream requirements, not by us: generic-array 0.14.7, matchit 0.8.4, and toml 0.8.2 / toml_datetime 0.6.3 / toml_edit 0.20.2 (the GTK build scripts); plus wry and tao, pinned by `tauri-runtime-wry` [group].

### 5.8 On the current branch tip `f5199123` [measured]

- The 8 commits `eb3d0ffa..f5199123` touch six files: `connectors/common/{mod,tests}.rs`, `discord/connector.rs`, `core/src/utils/text.rs`, the GUI's `hidden-chars.json` and `docs/Daemon_Manual.md`. None of them is among the combined diff's 37 files.
- `git apply --check` of `combined.diff` on a `git archive f5199123` export: clean.
- `cargo +1.98.1 check --locked --workspace --all-targets --exclude openalpaca_gui` there: exit 0, 30 s (`int-tip-check-locked.log`).
- Full test suite there (`int-tip-test-full.log`, `--locked`, 294 s): **4186 passed, 0 failed, 13 ignored**. The 17 extra tests are the tip's own (connectors 95 → 107, core 2024 → 2029).
- The tip's new escaping code (`utils/text.rs`, `connectors/common`, `discord/connector.rs`) therefore compiles and passes on the combined dependencies and Rust 1.98.1.
- Its escaping sets are our own tables, held equal to the `regex` crate's `\p{Cf}`, `\p{Cc}` and `\p{Default_Ignorable_Code_Point}` by tests (`crates/openalpaca_core/src/utils/text.rs:67,349,643` at `f5199123`) [our-code].
- `regex`, `regex-syntax` and `regex-automata` are unchanged between HEAD's lock and the combined lock. Also unchanged: `serde`, `serde_json`, `chrono`, `blake3` and `unicode-normalization`; `uuid` moves 1.26.0 → 1.26.1 [measured, `lockdiff`].
- Rust 1.98's own `Debug` escaping of default-ignorables (toolchain report §5) is a separate path. No tip test failed on 1.98.1.
- Two orphaned `sleep 3600` from this run (started 15:58:28 and 15:58:38, with 0 before the run) were killed.
- **The plan can therefore be rebased onto the current tip without edits.**

## 6. Risks at the combined level (persisted data, secrets, wire formats, user-visible)

Each group's risk section stands. What integration adds or confirms:

- **Secrets [measured, combined].**
  - The aes-gcm 0.10 fixture (`secret_encrypted` written by HEAD's construction) decrypts on the combined state (s4, replay 11).
  - The keyring 3 → 4 keychain read-back is **source-reading only**, as in the secrets report. The worst case is "keys unavailable until rollback", not loss, because nothing deletes keychain entries on read.
  - The combination does not change this.
- **Persisted hashes [measured, combined].**
  - The `file_assets.sha256` spelling (new known-answer test), memory owner keys, partition keys, note revisions and the HOME_README digest pass after sha2 0.11 in combination (s4, replay 10).
  - blake3 is untouched.
- **Database [measured, combined].**
  - The schema-42 refusal test and the storage suite pass on SQLite 3.53.2 in combination (s4, replay 06).
  - The storage group's cross-version file probe (3.53 ↔ 3.51, WAL replay) was **not re-run** here [group].
- **Config files.**
  - toml 1.1 / toml_edit 0.25.15 config tests pass in the full suite [measured].
  - A round trip of the owner's real `~/.openalpaca/config/*.toml` is **not done** (rule). It remains the one pre-merge check for `llm.toml`, where a parse refusal would make `write_config` replace the file.
- **Wire formats and user-visible behaviour** (the union of the group reports; nothing new from combining):
  - the MCP protocol version offered (2025-11-25) and the `MCP-Protocol-Version` header;
  - CORS responses without `Vary`;
  - SQLite's 17-digit REAL→TEXT (no SQL of ours reaches it);
  - upload sniffing: infer 0.22 refuses some `text/*` uploads that pass today (owner decision D1);
  - XLSX cell text: whitespace trim, rich runs, `_x000D_`;
  - the pdf-extract Form-XObject font-scope regression (D3);
  - bzip2 and LZMA DOCX entries unreadable with zip `deflate` only (D2);
  - Rust 1.98 `Debug` escaping of variation selectors;
  - React 19.3, the Vite 8 CSS minifier (checked only in Chromium), and the REPL's signal-handler lifetime (rustyline 18; needs a TTY check).
- **Build platforms.**
  - After the combination, Linux needs no `libdbus-1-dev` for the daemon or CLI, but still needs OpenSSL headers (teloxide native-tls) [measured graph].
  - No Linux or Windows compile happened here. The ubuntu CI run of commit 12 onward is the first Linux build of zbus/keyring 4, and of sysinfo 0.39's Linux paths.

## 7. Artefacts

- `SP/patches/combined.diff`: the full combined state against `5e5ac317` (37 files). **Compiles**; tests 4169/0/13. Applies cleanly to `5e5ac317` and to `f5199123`, where the tests are 4186/0/13.
- `SP/patches/combined-commits/01…28-*.diff`: the replayed commit series (git format, first line `# NN title`). They apply in order; each Rust commit compiles.
- `SP/int/replay.sh`, `SP/int/replay/summary.tsv`, `SP/int/replay/snap-NN/`: the replay driver, the per-commit results and the file snapshots.
- `SP/int/c.sh`, `rep.py`, `lockdiff.py`, `dupcount.py`, `fmtfix.py`, `mkdiff.sh`: the helpers.
- Logs: `SP/logs/int-*.log` (`s*` steps, `rp-*` replay, `tip-*` branch tip, `tree-*`, `deny-*`, `audit-*`).
- The worktree `SP/wt-combined` holds the combined state, plus gitignored `node_modules/`, `dist/` and the empty sidecar placeholder `apps/openalpaca-gui/src-tauri/bin/openalpacad-aarch64-apple-darwin`.

## 8. Not verified here (and who has to)

| Item | Why not | Owner / next step |
|---|---|---|
| Linux build and tests (zbus/keyring 4, sysinfo 0.39 Linux paths, `libdbus-1-dev` removal) | No Linux target here | The ubuntu CI job on the PR |
| Windows (keyring's windows-native store, Pester 6.2.0) | No Windows | The windows CI job; manual for keyring |
| keyring 3 → 4 read-back of an existing entry | Rule: no real keychain | A disposable macOS user or VM; a Linux container with gnome-keyring |
| The owner's real config files through toml 1.1 / toml_edit 0.25.15 | Rule: no `~/.openalpaca` | A read-only, no-print equality check (config group §3.1) |
| Live Telegram/Discord, MCP servers over npx, the REPL on a TTY, `tauri dev` in WKWebView, a packaged `tauri build` | Network, TTY or GUI, and the real store | The group reports' manual checks, each on an isolated `OPENALPACA_HOME_STORE` |
| The documents group's characterisation tests and web's CORS test | Not written by those groups | Before commits 13 and 20 |
| The SQLite cross-version probe and the 100K benches on the combined state | Not re-run | Optional; storage group §3.5 |

Owner decisions carried unchanged from the groups:
- serde_yaml option A/B/C (C is in the diff);
- infer D1, zip D2, pdf-extract D3;
- the teloxide rustls flag;
- rmcp default features;
- tower-http 0.7 vs 0.6 and tokio-tungstenite 0.30 vs 0.29;
- the plugin-react swap;
- the Bun version (1.4.2 ⇒ lockfile v2 when re-resolved);
- Pester 6.2.0 vs 5.9.1;
- Dependabot alerts on;
- whether to take the optional clippy commit;
- landing after L8.
