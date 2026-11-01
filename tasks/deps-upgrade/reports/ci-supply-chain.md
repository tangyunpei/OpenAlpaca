# CI, supply chain and repository hygiene — research for the toolchain and dependency bump

Group: CI, supply chain, repository hygiene. Date: 2026-09-25. Repository state read: `feat/markdown-memory` @ `eb3d0ffa` (main checkout, read-only). CI evidence: the latest `main` run, `35911776148` on `4256b77c` (2026-09-23).

Evidence tags: **[measured]** = I ran it (command and log named); **[code]** = read in our code (file:line); **[dep-src]** = read in dependency source (path or repo@tag); **[docs]** = changelog, release notes or documentation (URL); **[not established]** = could not be settled.

Patch: `SP/patches/ci-supply-chain.diff` (applies cleanly to HEAD — `git apply --check` in a fresh export, [measured]). It holds the CI changes, the dead lockfile's removal, `deny.toml`, `audit.yml`, `dependabot.yml` and the Bun pin. There is no Rust code in it, so "compiles" does not apply. The YAML parses (Ruby `YAML.load_file`, [measured]). zizmor 1.30.1 reports 13 findings on today's `ci.yml` and none on the proposed workflows [measured]. **The workflows themselves can only be verified by a CI run on GitHub.**

---

## 0. Summary

1. **CI's Rust pin is already a dead copy.** `rust-toolchain.toml` overrides it: the CI log says the toolchain is "overridden by …/rust-toolchain.toml" [measured, CI log line 301]. Replace `dtolnay/rust-toolchain@master` + `toolchain: 1.93.0` with `rustup toolchain install` (no argument). rustup 1.29.1 is on the runner image, and with no argument it installs the channel, profile and components the toml names [measured: `rustup toolchain install --help` on rustup 1.29.1; docs: rustup CHANGELOG 1.28.0]. That leaves one pin, and it also drops a third-party action that tracks a moving branch.
2. **`actions/checkout@v4` runs on a removed runtime.** It targets Node 20, which the runners removed on 2026-09-23. Every job already carries "being forced to run on Node.js 24" [measured, CI annotations]. Latest is **v7.0.1**. v7's one behaviour change (refusing fork PR code under `pull_request_target`/`workflow_run`) does not touch our `push`/`pull_request` triggers [code: ci.yml:3-6]. rust-cache v2.9.2 and setup-bun v2.2.0 are current and already on Node 24 [measured: action.yml `using: node24`].
3. **Bun floats in CI and is pinned nowhere.** CI installed **Bun 1.4.2** on 2026-09-23, while the machine that wrote `bun.lock` runs **1.3.13** [measured]. No `packageManager`/`engines` field exists [code]. Proposal: `"packageManager": "bun@1.4.2"` in the root `package.json`, which setup-bun reads by default [docs: setup-bun README@v2.2.0]. The field leaves `bun.lock` byte-identical [measured].
4. **Pester floats too.** CI never runs its `Install-Module` branch, because the image ships Pester 5.9.0. If an image ever ships Pester 6, `Import-Module -MinimumVersion 5.0.0` would load it silently [measured: CI log shows no install; image README lists `Pester: 3.4.0, 5.9.0`]. Proposal: an exact `-RequiredVersion`. Latest is **6.2.0**. Our test file uses nothing Pester 6 removed [code]. That is not verified: there is no PowerShell here.
5. **The `apps/openalpaca-gui/src-tauri/Cargo.lock` file is dead.** It is proved four ways [measured, plus dep-src]. Remove it.
6. **Advisories at HEAD:** 5 vulnerabilities, 7 unmaintained crates and 3 unsound findings. There are no yanked crates [measured: cargo-audit 0.22.2, 1271 advisories, DB `e2111519` of 2026-09-25]. **An all-latest resolution** (every planned bump, then `cargo update`, resolved but not compiled) **has 0 vulnerabilities**. It keeps 9 informational warnings, all transitive through tauri, teloxide or lopdf [measured].
7. **The bumps raise duplicates.** Before and after, measured with `cargo tree -d -e normal`:
   - Workspace, host: 33 → 38 duplicated crate names.
   - Workspace, all targets: 65 → 71.
   - Daemon binary: 13 → 20.
   - CLI: 9 → 2.
   - GUI: 10 → 15.

   The cause is the RustCrypto 0.10 → 0.11 generation split, plus two avoidable picks (§5).
8. **Maintenance recommendation.** Use Dependabot: monthly, one grouped PR per ecosystem, Cargo in `lockfile-only` mode, a 7-day cooldown, and SHA-pinned actions. Add a cargo-deny advisories workflow that also runs weekly. Dependabot alerts are **disabled** on this **public** repo today [measured: API 403 "Dependabot alerts are disabled"]; turning them on is a settings change only the owner can make.

---

## 1. What I ran

| What | Command (abridged) | Result / log |
|---|---|---|
| Install tools | `cargo +1.98.1 install --locked --root SP/tools cargo-audit` / `cargo-deny` / `zizmor` | cargo-audit 0.22.2, cargo-deny 0.20.2 (`logs/ci-install-*.log`) |
| Audit HEAD | `cargo-audit audit --db SP/tools/advisory-db --file <main>/Cargo.lock` (+ `--json`) | `logs/ci-audit-main.txt`, `.json` |
| Audit worktrees | same, on copies of `SP/wt-*/Cargo.lock` at 14:25 and 14:40 PDT | `logs/ci-audit-worktrees-round1.txt`, `-round2.txt` |
| All-latest resolution | export of HEAD → edit 17 requirements + `rust-version`/toolchain → `cargo +1.98.1 update` → `fetch` | `scratch-ci/latest/Cargo.lock` (786 packages vs 732), `logs/ci-latest-update.txt`, `logs/ci-audit-latest.txt` |
| cargo-deny gate trial | `cargo-deny --locked --all-features --workspace --config deny-proposed.toml check advisories` on HEAD and all-latest | `logs/ci-deny-proposed-{head,latest}.txt` |
| Duplicates | `cargo +1.98.1 tree --locked --offline -d --workspace -e normal [--target all]`; per binary `-p openalpacad/openalpaca/openalpaca_gui`; attribution via `scratch-ci/dups.py` over `cargo metadata` | `logs/ci-tree-dups-*.txt`, `logs/ci-dups-*.txt`, `scratch-ci/perbin-*.txt` |
| Dead-lockfile probe | in an export of HEAD: corrupt + `chmod 000` the src-tauri lock, then `locate-project`, `metadata --locked`, `tree`, `update`, `generate-lockfile` from inside `src-tauri`; then `tauri info` (CLI 2.11.4) | `logs/ci-deadlock-probe.txt`, `logs/ci-tauri-info.txt` |
| Bun probes | in a scratch copy of the three JS manifests: `bun install --frozen-lockfile --lockfile-only` with a garbage / absent `pnpm-workspace.yaml`, and with `packageManager` added | `logs/ci-bunprobe*.txt` |
| Workflow lint | `zizmor 1.30.1 --offline` on today's ci.yml and on the proposed ci.yml/audit.yml/dependabot.yml | today: **13 findings** (6 high `unpinned-uses`, 4 medium `excessive-permissions`, 3 medium `artipacked`); proposed: **0** (`logs/ci-zizmor-{orig,proposed}.txt`) |
| CI evidence | `gh run view --job <id> --log` for the three jobs of run 35911776148; check-run annotations; runner-image READMEs at the logged image versions | `logs/ci-gh-job-*.log`, `scratch-ci/ubuntu2404-readme.md`, `win2025-readme.md` |
| Upstream facts | `gh api` on releases/tags/action.yml of each action, rust-lang/rust `RELEASES.md` and `src/version@beta`, tauri-cli source @ `tauri-cli-v2.11.4` | quoted below |

Every cargo call used `nice -n 10 env CARGO_TERM_COLOR=never CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=SP/target-ci cargo +<toolchain>`. Nothing ran in the main checkout. No cargo command wrote under it: every command ran in exports or in other groups' worktrees, with `--locked`/`--offline` for read-only use.

---

## 2. GitHub Actions (task 1)

### 2.1 Per action

| Action | In use (ci.yml line) | Resolves to [measured, CI log] | Runtime | Latest (2026-09) | Breaking changes between | Recommendation |
|---|---|---|---|---|---|---|
| `actions/checkout` | `@v4` (18, 45, 69) | `11d5960a…` | **node20** — "forced to run on Node.js 24" warning on all 3 jobs [measured, annotations] | **v7.0.1** (2026-07-20) = `3d3c42e5aac5ba805825da76410c181273ba90b1` | v5: node24 runtime, runner ≥ 2.327.1. v6: persisted credentials live in a separate file under `$RUNNER_TEMP`. v7: refuses fork-PR checkout under `pull_request_target`/`workflow_run` unless `allow-unsafe-pr-checkout`, and moves to ESM [docs: README@v7.0.1, releases]. The same refusal was backported as "BREAKING" in v4.4.0, v5.1.0 and v6.1.0 | Move to v7.0.1. Our triggers are `push` + `pull_request` [code: ci.yml:3-6], so v7's change does not apply. The runner is 2.337.0 [measured]. Nothing runs authenticated git after checkout [code] |
| `dtolnay/rust-toolchain` | `@master` + `toolchain: 1.93.0`, `components: clippy` (22-25) | `02cb101e…` (a **moving branch**) | composite | n/a (branch). It has `@1.98.1`-style branches [measured: `refs/heads/1.98.1`, `1.99.0`] | It cannot read `rust-toolchain.toml`; `toolchain` is required unless the ref names the version [docs: README] | **Remove it** and use `rustup toolchain install` (§2.2) |
| `Swatinem/rust-cache` | `@v2` (26) | `6323deb1…` = v2.9.2 | node24 | **v2.9.2** (2026-08-06) | v2.9.0: node24, and the key includes **all** installed toolchains. v2.9.2: supports Cargo's v2 build-dir layout [docs: releases] | Pin SHA `6323deb102c322ba6fcbdcafc7e3dddab59af2b6 # v2.9.2`. Caveat below |
| `oven-sh/setup-bun` | `@v2`, no `bun-version` (46) | `0c5077e5…` = v2.2.0 | node24 | **v2.2.0** (2026-03-14) | v2.2.0: node24 runtime. By default the version comes from `package.json` → `packageManager`, then `engines.bun`, else **latest** [docs: README@v2.2.0] | Pin SHA `0c5077e51419868618aeaa5fe8019c62421857d6 # v2.2.0`; pin Bun through the root `package.json` |
| Pester | `Install-Module -MinimumVersion 5.5.0` / `Import-Module -MinimumVersion 5.0.0` (72-76) | image `windows-2025-vs2026` 20260907.229 ships Pester **3.4.0, 5.9.0**, so the install branch never runs [measured: log has no install output, 15 tests pass; image README] | PowerShell 7.6.5 [image README] | **6.2.0** (2026-09-09). 5.x is maintained: 5.9.1 (2026-08-11) | Pester 6 removals [docs: release 6.0.0]: PS < 5.1/7.4, `Assert-MockCalled`, `-Pending`, `-Focus`, mock fall-through, the Legacy parameter set. `-ForEach @()` now throws. Discovery runs per file. Names expand only inside `<…>`. Our file uses none of these (no `Mock`, `-ForEach`, `-TestCases`, `Pending` or `-Focus`) [code: `scripts/release/tests/WindowsScripts.Tests.ps1`, 199 lines] | Pin `-RequiredVersion 6.2.0`. **[not established]** that the suite passes on 6.2.0 (no pwsh here); the CI run is the test. Fallback: 5.9.1 |

**Tag or SHA.** GitHub's hardening guide calls a full-length commit SHA the only immutable reference [docs: https://docs.github.com/en/actions/reference/security/secure-use#using-third-party-actions]. I could not establish whether these releases are marked immutable.

**Recommendation: pin every action by SHA with a `# vX.Y.Z` comment, and let Dependabot's `github-actions` ecosystem move them.** The pairing matters. A SHA pin with no updater rots silently, and the v4/Node 20 warning is exactly what that looks like. If the owner declines Dependabot, use major tags (`@v7`, `@v2`) instead, so security fixes flow in. The repo is **public** [measured: `gh repo view` → PUBLIC], and anyone's fork PR runs this workflow with a read-only token. The token is already read-only by default [measured: log "GITHUB_TOKEN Permissions: Contents: read, Metadata: read, Packages: read"]; `permissions: contents: read` states that explicitly. `persist-credentials: false` on every checkout keeps the token out of `.git/config`; no step after checkout uses git credentials [code]. With both, zizmor's findings go from 13 (6 high) to 0 [measured].

**The rust-cache caveat** [measured]. Since v2.9.0 the cache key lists every installed toolchain. The key block shows `1.93.0` **and** `1.98.1`, the image's own stable. A runner-image update that moves the image's stable Rust (1.99.0 is in beta [measured: `src/version@beta` = 1.99.0], due about 2026-10-01 on the six-week train [inferred]) therefore changes the key and forces a cold-ish restore. The last run already restored with `full match: false` [measured]. This costs time only. Setting `add-rust-environment-hash-key: false` avoids it [docs: v2.8.2 release], but I would not bother.

### 2.2 One source of truth for Rust

Today there are two pins: `rust-toolchain.toml:2` (`channel = "1.93.0"`) and `ci.yml:24`. The toml already wins in CI [measured, log line 301: "the toolchain '1.93.0-x86_64-unknown-linux-gnu' is currently in use (overridden by '…/rust-toolchain.toml')"].

Every other consumer already goes through rustup and the toml:
- `scripts/release/package-{macos,linux}.sh:48/55` and `package-windows.ps1:58` run plain `cargo`/`rustc` [code].
- `apps/openalpaca-gui/scripts/prepare-sidecar.ts:27,60-64` runs `rustc -vV` and `cargo build` [code].
- `bunx tauri build` runs cargo inside `src-tauri`, and rustup walks up to the toml [measured: `rustup show active-toolchain` in `src-tauri` of an export reports "overridden by …/rust-toolchain.toml"].

So the fix is to delete the CI copy:

```yaml
- name: Install Rust toolchain
  run: |
    rustup toolchain install
    rustup show active-toolchain
```

`rustup toolchain install` with no argument means "Install or update … by default the active toolchain" [measured: help text of rustup 1.29.1, the same version as the image's "Rustup 1.29.1"]. The toml's `profile = "default"` and `components = ["rustfmt","clippy"]` come with it, so `clippy` stays available. It also downloads `rust-docs`, which the action's `--profile minimal` skipped: a few seconds [not measured].

Consider the plan's Rust commit if CI's pin were left behind at 1.93.0 while the toml moved to 1.98.1. CI would install 1.93.0 for nothing, and the first `cargo` would auto-install 1.98.1 from the toml. rustup ≥ 1.28.1 auto-installs by default [docs: rustup CHANGELOG 1.28.1]. It would pass, just slower and more confusingly. Deleting the pin removes the trap.

`actions-rust-lang/setup-rust-toolchain` (v2.0.0) does read the toml. It is not recommended: another third-party action, and v1 sets `RUSTFLAGS=-D warnings` by default [docs, from memory of v1; not re-verified for v2 — **not established**]. That would turn today's 2 build warnings and 83 clippy warnings into failures (below).

### 2.3 How CI's floating Bun and the Rust pin interact with the plan

- **Rust.** The bump is one line in the toml plus `rust-version` (Cargo.toml:32). CI, the release scripts and the sidecar build follow automatically once the CI copy is gone. Pin the **exact** patch, `1.98.1`: three of the last three point releases fixed miscompilations — 1.96.1 (MIR), 1.97.1 (LLVM), 1.98.1 (vtables) [docs: rust-lang/rust RELEASES.md]. The brief dates 1.98.1 to 2026-09-01. `RELEASES.md` and the GitHub release say **2026-09-03**; the tauri-info rustc string says "2026-09-01", which is the commit date [measured]. 1.99.0 lands about 2026-10-01 [inferred], so decide explicitly whether the plan targets 1.98.1 or waits a week.
- **CI does not gate on warnings.** On main CI (Linux, 1.93.0):
  - The build emitted **2** warnings: unused imports at `apps/openalpacad/src/notification/imessage.rs:4`, a Linux-only cfg.
  - Clippy emitted **83** warnings.
  - All jobs were green; tests: 3477 passed, 0 failed, 6 ignored [measured: colour-stripped job log].

  New lints from 1.94–1.98 will therefore **not** fail CI; someone has to read the log. Cargo 1.97 stabilised `build.warnings`, meant for "enforcing a warning-free build in CI, replacing `-Dwarnings`" [docs: RELEASES.md 1.97.0 Cargo]. Using it is an owner decision, not part of this bump.
- **Bun.** Local installs use 1.3.13 [measured: `bun --version`]. CI used 1.4.2 on 2026-09-23 [measured: log "Downloading … bun-v1.4.2"], and will use whatever is latest on the day the plan's PR runs.
  - The JS group will regenerate `bun.lock` with local Bun and have it checked by a different Bun in CI. Bun 1.4.2 accepted today's `lockfileVersion: 1` lock under `--frozen-lockfile` [measured: CI green].
  - Bun 1.4 changes lockfile content only for GitHub/tarball dependencies (it adds sha512 integrity) [docs: https://bun.com/1.4]. We have none [code: bun.lock].
  - Risk today: low, but unbounded. A future Bun that rewrites the lock would turn CI red on no commit of ours. The precedent: `5e89c7e3` fixed a `--frozen-lockfile` refusal.

  Pin with `packageManager` and upgrade local Bun to the same version (`bun upgrade`, an owner action).
- **Runner images.** `ubuntu-latest` moves to **Ubuntu 26.04** over 2026-10-19 → 2026-11-19 [measured: CI notice annotation; docs: actions/runner-images#14748]. If the bump PR runs in that window, a failure could be the image rather than the bump. Pinning `runs-on: ubuntu-24.04` for the bump PR, then moving deliberately, removes the ambiguity. This is optional and not in the patch.

---

## 3. The dead `apps/openalpaca-gui/src-tauri/Cargo.lock` (task 2)

Facts:
- It is tracked, 5146 lines, added in `2d5e847b` (2026-02-01) and never touched since [measured: `git log`].
- It names the package `openalpaca-gui` and `tauri 2.9.5`, 487 packages [measured].
- The crate has been called `openalpaca_gui` since `5efbcc42`, the same day [measured: `git log -S`].
- It has been a workspace member since the very first commit [measured: `git show 2d5e847b:Cargo.toml`].

Proof it is never read or written:
1. **Cargo documentation.** "All packages share a common `Cargo.lock` file which resides in the workspace root" [docs: https://doc.rust-lang.org/cargo/reference/workspaces.html]. The only way to point Cargo at another lockfile is `resolver.lockfile-path`, stabilised in 1.97 [docs: RELEASES.md]. No config sets it [measured: grep of `.cargo/`, `~/.cargo/config*`].
2. **Measured in an export of HEAD** (`logs/ci-deadlock-probe.txt`), with the src-tauri lock overwritten by invalid TOML and `chmod 000`:
   - `cargo locate-project --workspace` from `src-tauri` names the root `Cargo.toml`.
   - `cargo metadata --locked --offline` from `src-tauri` exits 0.
   - `cargo tree -p openalpaca_gui` shows `tauri v2.11.5`: the root lock's version, not the dead lock's 2.9.5.
   - `cargo update -p tauri` and `cargo generate-lockfile` from `src-tauri` changed the **root** lock's mtime. The src-tauri file still held the garbage afterwards.
3. **Tauri CLI.** `tauri-cli` v2.11.4 reads `get_workspace_dir(tauri_dir).join("Cargo.lock")`, where `get_workspace_dir` is `cargo metadata`'s `workspace_root` [dep-src: tauri-apps/tauri@tauri-cli-v2.11.4 `crates/tauri-cli/src/helpers/cargo_manifest.rs:59-62`, `src/interface/rust.rs:1341-1347`]. I ran `tauri info` (the installed CLI 2.11.4) against the export with the corrupted file. It reported `tauri 🦀: 2.11.5` and wrote no file [measured: `logs/ci-tauri-info.txt`; `find -newer` found nothing].
4. **CI's rust-cache** lists the lockfiles it hashes: `…/Cargo.lock` (root), every member `Cargo.toml` and `rust-toolchain.toml` — not the src-tauri lock [measured: CI log lines ~430-445].
5. None of the tauri crates (`tauri`, `-build`, `-codegen`, `-macros`, `-utils`, `-plugin`, `-plugin-dialog`) mentions `Cargo.lock` [measured: grep in `~/.cargo/registry/src/…/tauri*`]. No repo script references it either [measured: `git grep 'Cargo.lock'`].

**Recommendation: `git rm apps/openalpaca-gui/src-tauri/Cargo.lock`** (in the patch). No `.gitignore` line is needed: Cargo never creates a member lockfile. Side effect: every tool that scanned it (Dependabot, GitHub's dependency graph, grep) stops seeing a phantom tauri 2.9.5 tree.

---

## 4. Supply chain (task 3)

### 4.1 HEAD (`<main>/Cargo.lock`, 732 packages)

cargo-audit 0.22.2 [measured, `logs/ci-audit-main.txt`]. Yanked crates: none (yanked checks are on by default). cargo-deny 0.20.2 agrees [measured].

| Advisory | Crate@version | Path to us [measured: `cargo tree -i`] | Reachable in our build? | Fixed by |
|---|---|---|---|---|
| RUSTSEC-2026-0187 (7.5) stack overflow on nested PDF objects | lopdf 0.34.0 | pdf-extract 0.8.2 → openalpacad | **Yes.** The daemon extracts text from uploads in-process: `extraction.rs:59-63` → `pdf_extract::extract_text`, called from `background.rs:136` [code]. A stack overflow aborts; `catch_unwind` cannot catch it [docs: advisory]. A ~21 KB crafted PDF upload would kill the daemon | pdf-extract 0.12.1 → lopdf 0.42.0 (wt-docs) [measured] |
| RUSTSEC-2026-0194, -0195 (7.5) quadratic attributes / unbounded NsReader | quick-xml 0.31.0 | calamine 0.26.1 → openalpacad | **Likely.** Spreadsheet uploads go through `extraction.rs:87-90` → `calamine::open_workbook_auto` [code]. Whether calamine calls the affected functions: **not established** | calamine 0.36.1 → quick-xml 0.41.0 (wt-docs) [measured] |
| RUSTSEC-2026-0189 (8.8) DNS rebinding, Streamable HTTP **server** | rmcp 0.16.0 | openalpaca_mcp | **No.** The code sits behind `transport-streamable-http-server(-session)` [dep-src: `rmcp-0.16.0/src/transport.rs:106-108`]. Our enabled features are client, child-process, streamable-http-client(-reqwest) and the defaults (server, macros), not the HTTP server transport [measured: `cargo tree -e features -i rmcp`]. It still fails any audit gate | rmcp 3.4.1 (wt-rmcp) [measured] |
| RUSTSEC-2026-0285 (5.3) TLS 1.3 messages accepted across encryption levels | rustls 0.23.43 | reqwest/hyper-rustls, twilight, teloxide … | Yes: every TLS client (LLM providers, connectors) | **Only the semver-compatible lock refresh** → 0.23.45 (in `SP/cargo-update-dryrun.txt`, also at MSRV 1.93). No group worktree had it at 14:40 |
| RUSTSEC-2026-0253, -0002 (unsound) | lru 0.12.5 | openalpaca_core (direct) | Practically no: no `iter_mut`, and keys are a plain enum without a panicking `Drop` [code: `compose/mod.rs:31,127-365`] | lru ≥ 0.18.2 → 0.18.5 (wt-storage) [measured] |
| RUSTSEC-2024-0429 (unsound) | glib 0.18.5 | gtk stack → tauri (Linux GUI only) | GUI on Linux | Not by us: needs glib ≥ 0.20, i.e. Tauri moving off gtk-rs 0.18 |
| RUSTSEC-2024-0370 / 2026-0173 (unmaintained) | proc-macro-error 1.0.4 / -error2 2.0.1 | glib-macros (Linux GUI) / aquamarine → teloxide 0.17.0 (latest) | build-time only | Not by us |
| RUSTSEC-2025-0075/-0080/-0081/-0098/-0100 (unmaintained) | unic-* 0.9.0 | urlpattern 0.3 → tauri-utils | GUI build | Not by us |

`serde_yaml 0.9.34+deprecated` (openalpaca_core) is **not** in the RustSec DB, so neither tool flags it [measured]. The deprecation is visible only in its version string. Replacing it is the config group's call, not an audit finding.

### 4.2 Worktrees (in progress; snapshots copied at 14:25 and 14:40 PDT)

Round 2 at 14:40 [measured, `logs/ci-audit-worktrees-round2.txt`]:
- **wt-docs** clears lopdf and both quick-xml advisories, and **adds** `ttf-parser 0.25.1` (unmaintained, RUSTSEC-2026-0192, via lopdf 0.42).
- **wt-rmcp** clears rmcp.
- **wt-storage** clears lru.
- **wt-deps** changes only tauri 2.11.6 and the plugin crates.
- **wt-web** and **wt-secrets** change nothing advisory-relevant.
- wt-secrets' `keyring = "4"` adds **65 packages net +53**, the zbus async stack.
- **No worktree had rustls 0.23.45 yet.**
- wt-combined and wt-config were still identical to HEAD.

Round 3 at 14:46 PDT [measured]: **wt-config** replaces `serde_yaml 0.9.34+deprecated` (and `unsafe-libyaml`) with `serde-saphyr 1.3.0`, and moves toml to 1.1.6 and toml_edit to 0.25.15. That adds 6 packages and no advisory; the lock still has the 5 baseline vulnerabilities. **At 14:46 no worktree lock carries rustls 0.23.45** (all are at 0.23.43), and wt-combined is still identical to HEAD.

### 4.3 All-latest endpoint [measured]

I applied every bump the groups are exploring to an export of HEAD:
- base64 0.23, aes-gcm 0.11, sha2 0.11, keyring 4 (no features, as wt-secrets), rusqlite 0.40.2, tower-http 0.7.1, tokio-tungstenite 0.30, rustyline 18, infer 0.22, pdf-extract 0.12, zip 8, calamine 0.36, lru 0.18, toml 1.1, sysinfo 0.39, rmcp 3.4.1;
- `rust-version = "1.98"`, toolchain 1.98.1;
- then a full `cargo update`.

Result:
- **cargo-audit: 0 vulnerabilities, exit 0.** 9 allowed warnings remain: unmaintained proc-macro-error, proc-macro-error2, ttf-parser and 5× unic-*, plus unsound glib.
- **cargo-deny with the proposed `deny.toml`** (`unmaintained`/`unsound = "workspace"`, `yanked = "deny"`, empty ignore list): **fails on HEAD with 7 errors** (5 vulnerabilities + 2 lru unsound) and **passes on the all-latest lock** (`advisories ok`).

This is a resolution, not a build: it shows which advisories remain, not that the code compiles.

**Consequence for ordering.** The advisories workflow must land **after** rmcp, pdf-extract, calamine, lru and the lock refresh. Otherwise it needs a reasoned `ignore` list for each advisory not yet fixed. RUSTSEC-2026-0189 is the one that can be ignored honestly (unreachable, §4.1).

---

## 5. Duplicate crate versions (task 4)

Measured with `cargo +1.98.1 tree --locked --offline -d -e normal`. The before state is `SP/wt-deps`, whose Cargo.lock equalled HEAD when read; the after state is the all-latest export. Attribution to the first external dependency comes from `scratch-ci/dups.py` over `cargo metadata`.

| Scope | Before | After all bumps |
|---|---|---|
| Workspace, host (aarch64-apple-darwin) | 33 names / 71 versions | 38 / 80 |
| Workspace, all targets | 65 / 151 | 71 / 160 |
| `openalpacad` (daemon) | 13 | **20** |
| `openalpaca` (CLI) | 9 | **2** |
| `openalpaca_gui` | 10 | **15** |

Workspace-wide numbers mix binaries that never link together (tauri never enters the daemon). The per-binary rows are the ones that cost compile time and size.

Before, by cause (host, `logs/ci-dups-mac.txt`):
- **Our direct pins, fixable by the planned bumps:**
  - nix 0.30 (rustyline 17) vs 0.31 (ours) — fixed by rustyline 18.
  - foldhash 0.1 and hashbrown 0.15 (lru 0.12) — fixed by lru 0.18.
  - toml 0.9 / toml_datetime 0.7 / winnow 0.7 (ours) vs toml 1.1 (tauri) — fixed by toml 1.1.
  - constant_time_eq 0.3 (zip 2/calamine) vs 0.4 (blake3) — fixed by zip 8.
  - infer 0.16 (ours) vs 0.19 (tauri) — not fixed by 0.22; see below.
  - tokio-tungstenite/tungstenite 0.28 (ours) vs 0.29 (axum) — see below.
  - core-foundation/security-framework (keyring 3, non-host) — fixed by keyring 4.
- **tauri:** base64 0.21, bitflags 1, indexmap 1 / hashbrown 0.12, miniz_oxide 0.8, png 0.17 vs 0.18, phf 0.13, schemars 0.8/0.9/1, thiserror 1, derive_more 2, quick-xml 0.41 (plist), the Windows `windows-*` 0.61 generation, proc-macro-crate 1/2/3, toml_edit 0.19/0.20, jni 0.21.
- **teloxide 0.17 (latest):** reqwest 0.12 vs our 0.13, derive_more 1, bitflags 1, hashbrown 0.12/indexmap 1, foreign-types 0.3, wasm-streams 0.4.
- **tokio-cron-scheduler 0.15.1 (latest):** darling 0.20, phf 0.12.
- **dashmap 6.2.1 (latest):** hashbrown 0.14.
- **twilight 0.17 / rusqlite 0.38:** hashbrown 0.16.
- **calamine 0.26:** quick-xml 0.31.
- **rmcp 0.16:** darling 0.23. rmcp 3.4.1 moves to 0.24 while tauri/teloxide are on 0.23/0.24; the darling 0.20 left is tokio-cron-scheduler's.
- **Ecosystem-wide:** syn 2 vs syn 3 (dozens of proc-macros); getrandom 0.2/0.3/0.4 and rand_core 0.6/0.9/0.10 (axum's tungstenite on rand 0.9).

After, by cause (`logs/ci-dups-latest-mac.txt`, `cargo tree -i`):
- **Resolved:** constant_time_eq, foldhash, nix, toml, toml_datetime, winnow; on all targets also core-foundation and security-framework.
- **New: the RustCrypto 0.10 → 0.11 generation split**, 9 names (aes, block-buffer, block-padding, cipher, cpufeatures, crypto-common, digest, inout, sha1/sha2).
  - The 0.11 side: our aes-gcm 0.11 and sha2 0.11, **and** calamine 0.36 / zip 8 regardless of what we do.
  - The 0.10 side: lopdf 0.42 (aes/cbc/ecb for encrypted PDFs, sha2 0.10), axum 0.8.9 and tungstenite 0.29 (sha1 0.10), tauri-codegen (sha2 0.10).
  - Unavoidable in the daemon once zip 8/calamine 0.36 land.
  - In the **GUI** the split comes only from our `sha2 0.11` through `openalpaca_storage`: 5 of its 5 new duplicates.
- **New and avoidable:**
  - **tower-http 0.7.1 (ours) vs 0.6.11** (reqwest 0.12 and 0.13.5). Only the `cors` feature is used [code: Cargo.toml:64]. Holding tower-http at 0.6 until reqwest moves avoids a duplicate in the daemon. No advisory forces 0.7.
  - **tokio-tungstenite 0.30 (ours) vs 0.29 (axum 0.8.9).** Choosing **0.29** would unify tokio-tungstenite + tungstenite with axum: 2 names fewer in the daemon.
  - infer 0.22 (ours) vs 0.19 (tauri) matters only workspace-wide: tauri is not in the daemon.
- **base64:** our 0.23 joins reqwest 0.13.5/rmcp/hyper-util. 0.22 remains through axum, reqwest 0.12 (teloxide) and tokio-websockets (twilight). Neutral.

These are recommendations for the web and secrets groups to weigh, not requirements. Duplicates cost compile time, not correctness.

---

## 6. Every version citation that moves with the bump (task 5)

[code; `git grep` over the tree excluding `tasks/`, `*.lock`, `docs/api/`, fixtures]

| File:line | Text today | Changes when | Owner group |
|---|---|---|---|
| `rust-toolchain.toml:2` | `channel = "1.93.0"` | Rust bump → `"1.98.1"` | toolchain |
| `Cargo.toml:32` | `rust-version = "1.93"` | Rust bump → `"1.98"` (MSRV policy, §7.3) | toolchain |
| `.github/workflows/ci.yml:22-25` | `dtolnay/rust-toolchain@master`, `toolchain: 1.93.0` | deleted (patch) | CI |
| `.github/workflows/ci.yml:18,45,69` / `:26` / `:46` / `:72-76` | action refs and Pester | patch | CI |
| `.github/workflows/ci.yml:19-21` | "keyring's secret-service backend links the system dbus-1 library" + `apt-get install libdbus-1-dev pkg-config` | **Becomes stale if keyring 4 lands with default features.** In the all-latest lock `libdbus-sys` remains only via tao → tauri, i.e. the excluded GUI crate [measured: `cargo tree --target x86_64-unknown-linux-gnu -i libdbus-sys`]. Verify by dropping the step in that PR's CI | secrets + CI |
| `README.md:26` | "The repo pins 1.93.0 in `rust-toolchain.toml`" | Rust bump | docs/toolchain |
| `README.md:29` | Linux prereq `libdbus-1-dev pkg-config` | only if keyring 4 removes the need; the GUI still needs Tauri's libs | secrets |
| `README.md:205` | "CI … has three jobs" | if `audit.yml` lands: add "and a separate advisories workflow" | CI |
| `README.md:208` | "Rust 1.93.0. Frontend: Bun, TypeScript 7, React 19, Vite 7, Tailwind 4, Vitest 4." | Rust, and Vite 8 / Vitest 5 **if** the JS group takes those majors | docs/JS |
| `CLAUDE.md:152` | "Rust 1.93.0 … TypeScript 7.0.2, React 19.2, Tailwind 4.3, Vite 7, Vitest 4, Bun." | Rust 1.98.1, React 19.3 (wt-deps), Vite/Vitest per JS, "Bun 1.4.2 (root `packageManager`)" | docs |
| `CLAUDE.md:154` | CI command list (`cargo build\|test --workspace …`) | add `--locked`, the advisories workflow, and "the toolchain comes from rust-toolchain.toml" | CI |
| `apps/openalpaca-gui/README.md:14` | "React 19, TypeScript 7 …, Vite 7" | only on a Vite major | JS |
| `apps/openalpaca-gui/README.md:31-32` | Bun / "Rust toolchain (pinned by the workspace `rust-toolchain.toml`)" | optional: name the `packageManager` pin | JS |
| `apps/openalpaca-gui/README.md:71` | "The `gui` job in ci.yml runs these four" | unchanged | — |
| `apps/openalpaca-gui/DESIGN_SPEC.md:4`, `apps/openalpaca-gui/package.json:4`, `docs/GUI_Manual.md:3-4`, `CLAUDE.md:164`, `README.md:138`, `apps/openalpaca-gui/README.md:3` | Tauri 2 / React 19 / TypeScript 7 / Tailwind 4 (majors only) | only on a major change | — |
| `apps/openalpaca-gui/tsconfig.json:29` | "TypeScript 7 removed it" | unchanged | — |
| `docs/Installation_Manual.md:35-36`, `:592-596` | `cargo`/`rustc`, `bun`/`bunx`, "PowerShell 5.1 or later" — no versions | unchanged | — |
| `docs/QuickStart_Manual.md:7,16` | "Rust toolchain", "`cargo`, `bun` and `git`" — no versions | unchanged | — |
| `scripts/release/package-{macos,linux}.sh:26-35/19-28`, `package-windows.ps1:1,33-35` | `require_cmd cargo/rustc/bun/bunx`, `#Requires -Version 5.1` — no version pins or checks | unchanged. Optional hygiene: `cargo build --locked` (`:55`/`:48`/`:58`) and `bun install --frozen-lockfile` (`:60`/`:53`/`:64`) so a release never re-resolves | CI/hygiene |
| `apps/openalpaca-gui/src-tauri/tauri.conf.json` | no `bundle.macOS.minimumSystemVersion` (Tauri default) | unchanged: RELEASES.md 1.94–1.98 has no platform-minimum change (only Tier-2 promotions of tvOS/watchOS/visionOS and thumb targets) [docs] | — |
| `.cargo/config.toml:1-2` | `[resolver] incompatible-rust-versions = "fallback"` | unchanged; redundant with `resolver = "3"`, whose default it is [docs: Cargo resolver v3] | — |
| `scripts/eval/memory/reports/*-2026-09-24.json:12` | `"rustc": "rustc 1.93.0 …"` | **must not change**: historical run records | — |

No Rust source file cites a toolchain version [measured: `git grep` in `crates/`, `apps/`].

---

## 7. Ongoing maintenance (task 6)

### 7.1 Dependency updates — options

| Option | What it does | Cost / caveats |
|---|---|---|
| **A. Dependabot (recommended)** — `.github/dependabot.yml` in the patch | cargo `versioning-strategy: lockfile-only` + `allow: dependency-type: all` + one group gives a **monthly Cargo.lock refresh PR** (transitive crates included, e.g. rustls). It never edits a requirement, so 0.x "minor" breaks cannot arrive. bun: one grouped minor/patch PR, majors ignored. github-actions: moves the SHA pins. `cooldown: default-days: 7` | Native; no third-party app with repo write access. Caveats [docs]: for cargo only `lockfile-only`/`auto` are accepted; Dependabot **does not do security updates for bun**; `allow: all` includes transitive dependencies for cargo but not bun. A Dependabot lock refresh is not byte-identical to `cargo update` [inferred]. Majors stay hand-planned, as this plan does |
| B. Renovate | `lockFileMaintenance` is literally `cargo update` + `bun update` in one PR. `minimumReleaseAge`, a Dependency Dashboard | Needs the Renovate GitHub App (a third party with write access) or self-hosting; more config surface |
| C. Neither; a documented quarterly "refresh" chore | a manual `cargo update` + `bun update` + actions bump | Relies on memory. The Node 20 episode shows what happens |

Variant of A: `multi-ecosystem-groups` (GA) can merge all three into **one** PR [docs: GitHub changelog 2025-07-01]. I kept one PR per ecosystem so a red Bun update cannot block the Rust lock refresh.

Separately, **turn on Dependabot alerts** (Settings → Code security). They are disabled on this public repo [measured]. That is an owner-only setting.

### 7.2 Advisory gate

Recommended: **cargo-deny, advisories only**, in its own workflow (`audit.yml`: push to main, PRs, and weekly on Monday 06:17 UTC). The weekly run matters: an advisory published against an unchanged lock still turns red.
- The action is `EmbarkStudios/cargo-deny-action@3c6349835b2b7b196a839186cb8b78e02f7b5f25 # v2.1.1`, a Docker action shipping cargo-deny 0.20.2 [dep-src: Dockerfile@v2.1.1]. Its entrypoint has a workaround for rustup ≥ 1.28 and `rust-toolchain.toml` [dep-src: entrypoint.sh@v2.1.1].
- The policy (`deny.toml`) is `unmaintained = "workspace"` and `unsound = "workspace"`, so transitive crates we cannot fix do not fail the gate. `yanked = "deny"`. The ignore list is empty and must state reasons.
- Measured behaviour: fails on HEAD with 7 errors; passes on the all-latest lock.
- The alternative is `cargo install --locked cargo-audit` in the Rust job: about 2 min 10 s to compile here [measured], and it reports transitive unmaintained crates as warnings. `rustsec/audit-check` was last released 2024-09 [measured].
- Licenses, bans and sources are **not** proposed. The license inventory has ~40 distinct expressions, including 6 MPL-2.0 crates [measured: `cargo metadata`]. An allow-list is a separate decision.

### 7.3 MSRV policy

- The workspace is `publish = false` [code: Cargo.toml:35]. `rust-version` does two things only:
  - it drives the MSRV-aware resolver (resolver 3's default `fallback`, also set explicitly in `.cargo/config.toml`);
  - it documents the minimum.
- Today it holds back exactly **one** semver-compatible update: takecell 0.1.2, which requires Rust 1.96. Plus sysinfo 0.39, which is a semver-major anyway [measured by the orchestrator: `SP/cargo-update-dryrun.txt` vs `-nomsrv.txt`].
- **Recommendation: `rust-version` = the pinned toolchain's minor** (`"1.98"` with `channel = "1.98.1"`), bumped together, deliberately, on a cadence the owner picks — for example every second or third release, not every six weeks. There is no downstream to keep compatible, and a lower MSRV would only make the resolver pick older crates.

---

## 8. Other hygiene found (not forced by the bump; each optional)

- **`rust_out`** is a tracked 463 KB Mach-O arm64 executable at the repo root, added in `96ebdc16` ("fix: issues fix", 2026-02-23) [measured: `git ls-files -s`, `file`]. `rust_out` is rustc's default output name for code read from stdin. It is almost certainly an accident: `git rm`, plus `/rust_out` in `.gitignore`.
- **`pnpm-workspace.yaml`** dates from the init commit, when pnpm was the plan [measured: `git log`]. Bun 1.3.13 ignores it: `bun install --frozen-lockfile --lockfile-only` succeeds and leaves `bun.lock` byte-identical with the file absent or replaced by invalid YAML [measured]. Not re-measured with Bun 1.4.2. Its `onlyBuiltDependencies: [esbuild]` is pnpm-only. Candidate for removal.
- **The release scripts install without lock enforcement** (`cargo build` without `--locked`, `bun install` without `--frozen-lockfile`; §6 table). A release built from a tree whose lock is stale re-resolves silently.
- **CI is not warning-free and not gated on warnings** (§2.3). The CLAUDE.md claim "the workspace build is warning-free" does not hold on Linux (2 unused-import warnings on main CI).
- **CI does not run `cargo fmt --check`**, although the toml installs rustfmt. Noted, not proposed.

---

## 9. Risks to persisted data, secrets, wire formats, user-visible behaviour

- **Persisted data / secrets:**
  - Nothing in this group's patch touches the store, the database, `llm.toml` or the keychain.
  - Removing the dead lockfile changes no build [measured].
  - One cross-group flag, for the secrets group: wt-secrets moves `keyring` 3 → 4 **and drops the explicit backend features** (`apple-native`, `windows-native`, `sync-secret-service`) in favour of keyring 4's default `v1` feature (apple keychain, windows native, **zbus** secret-service) [dep-src: `keyring-4.2.0/Cargo.toml [features]`]. Whether keyring 4 reads entries keyring 3 wrote (same service/user naming, same backend) decides whether stored API keys are orphaned. **Not established by me.** It needs a test against a keychain written by the current build, never against the owner's real keychain.
- **Wire formats:**
  - CI changes none.
  - Pinning Bun 1.4.2 cannot rewrite `bun.lock` for this repo's dependency set (no GitHub/tarball dependencies) [docs + code]. 1.4.2 already accepts the current lock in CI [measured].
- **User-visible:**
  - Until pdf-extract/calamine move, a crafted PDF or XLSX upload can abort or stall the daemon (§4.1). The bump fixes it.
  - The GUI and CLI are unaffected by the CI changes.
  - If Pester 6.2.0 fails the suite, only the Windows job turns red; the fallback is 5.9.1.

---

## 10. Proposed commits, in order

| # | Title | Contents | Depends on | Verification |
|---|---|---|---|---|
| C1 | `ci: pin actions by SHA, take the Rust toolchain from rust-toolchain.toml` | ci.yml: checkout v7.0.1 (`persist-credentials: false`), rust-cache v2.9.2, setup-bun v2.2.0 by SHA; drop dtolnay; `rustup toolchain install`; `permissions: contents: read`; `--locked` on build/test/clippy; Pester `-RequiredVersion 6.2.0` (fallback 5.9.1). Root package.json `"packageManager": "bun@1.4.2"` | none; can land **before** the toolchain bump (it installs 1.93.0 from the toml today) | CI green on all 3 jobs. The Rust log shows "overridden by rust-toolchain.toml" and the channel equal to the toml; the gui log shows `bun install v1.4.2`; no "Node.js 20 is deprecated" annotation; the Windows log shows Pester 6.2.0 and "Tests Passed: 15". Locally, `git apply --check` passed and zizmor reports 0 findings [measured] |
| C2 | `chore: remove the src-tauri Cargo.lock that cargo never reads` | `git rm apps/openalpaca-gui/src-tauri/Cargo.lock` | none | `cargo metadata --locked` from `src-tauri` exits 0; `bunx tauri info` shows the root lock's tauri version (both [measured] in an export) |
| C3 | toolchain bump (toolchain group) | toml `1.98.1`, `rust-version = "1.98"`, README:26/208, CLAUDE.md:152 | C1 (else CI installs a dead 1.93.0 first) | CI green; the log shows 1.98.1 |
| C4… | dependency bumps (other groups) + **the semver-compatible `cargo update`** (the only thing that fixes rustls) | per group | C3 where MSRV requires | per group, plus `cargo-audit` on the resulting lock shows 0 vulnerabilities |
| C5 | `ci: advisories workflow (cargo-deny) + deny.toml` | `.github/workflows/audit.yml`, `deny.toml` | after rmcp, pdf-extract, calamine, lru and the lock refresh (otherwise add reasoned ignores) | `cargo deny --locked --all-features --workspace check advisories` passes locally [measured on the all-latest lock]; the workflow is green on the PR |
| C6 | `ci: Dependabot for cargo (lockfile-only), bun and actions` | `.github/dependabot.yml` | C1 (SHA pins to maintain) | GitHub → Insights → Dependency graph → Dependabot shows the three ecosystems with no config error; the first monthly run opens ≤ 3 PRs. Owner: enable Dependabot alerts |
| C7 | docs: version citations | §6 rows marked docs/CI | after the versions settle | `git grep -n '1\.93'` outside history/fixtures returns nothing |
| opt | hygiene | `git rm rust_out` (+ .gitignore), `git rm pnpm-workspace.yaml`, `--locked`/`--frozen-lockfile` in the release scripts | none | `bun install --frozen-lockfile` green in CI; `./scripts/release/package-macos.sh` still builds (owner machine) |

The patch file bundles C1, C2, C5 and C6 for review. Split it along the table when committing.

---

## 11. Open questions

1. **keyring 3 → 4 (secrets group).** Do entries written by keyring 3's `apple-native` / `windows-native` / `sync-secret-service` backends read back under keyring 4's default `v1` features? This must be tested on a throwaway keychain entry, never on the owner's keychain. If they do not read back, stored API keys are orphaned.
2. **Pester 6.2.0.** Does the Windows suite pass on it? No PowerShell is available here; the first CI run answers it (fallback 5.9.1).
3. **Rust version.** Target 1.98.1 now, or wait for 1.99.0, due about 2026-10-01 (in beta today)?
4. **Warnings in CI.** Should CI start failing on warnings (Cargo 1.97 `build.warnings`) once the toolchain group's clippy fixes land? This is an owner decision, not part of the bump.
5. **Two dependency picks for the web group.** tower-http 0.7 vs holding 0.6, and tokio-tungstenite 0.30 vs 0.29 to match axum. Both are duplicate-avoidance choices, not requirements.
6. **Owner settings.** Enable Dependabot alerts on the public repo. Upgrade local Bun to the pinned version.
7. **The `libdbus-1-dev` step.** Does it become removable from CI once keyring 4 (zbus) lands? Only a CI run without the step proves it.

