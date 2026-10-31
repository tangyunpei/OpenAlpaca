# Dependency upgrade plan audit — 2026-10-08

**Verdict: retain rev 2.1 as historical research; refresh the patches and safety gates before executing it.** The broad upgrade order remains useful. The preserved series is no longer ready to apply to the current application.

Audited `tasks/deps-upgrade/plan.md`, all 11 group reports, the preserved patches/tools/logs, and current source at **`d484a56a92aac4d7c5e78c29dd25e8761c28b60d`**, branch `feat/markdown-memory`. This report is a new file outside the original `tasks/deps-upgrade/` directory. The original plan, reports and archive were left unchanged.

## Current state and measured checks

The **application has evolved; the dependency wave has not landed**. Rust is still pinned to 1.93.0, with workspace MSRV 1.93. The original major-version starting requirements remain: rusqlite 0.38, sha2/aes-gcm 0.10, keyring 3, rmcp 0.16, toml 0.9, serde_yaml 0.9, and Vite 7/Vitest 4. The root package manifest still has no Bun version pin.

Since the plan's last checked tip, `908050f1`, the branch has gained **471 commits**, changing **538 tracked files**. **Seven** of the combined patch's 37 paths overlap those changes: `Cargo.lock`, core's manifest, builtins module and evaluation helper, key encryption, and two storage test files. Changes outside those seven paths also alter the behavior that the plan's gates must exercise.

| Check performed in this audit | Result and boundary |
|---|---|
| `git apply --check` of archived `combined.diff` against current HEAD | **Fails** in `Cargo.lock`, `storage/src/database/tests.rs`, and `storage/src/store/tests.rs`. No patch was applied to the working checkout. |
| Current-source metadata, Rust 1.98.1, offline/locked | Passes. This is manifest/workspace inspection, not compilation or a test result. |
| Transplant into a tracked-file export of current HEAD, excluding those three conflicting files | Remaining combined patch applies. Cargo regenerated the scratch lock offline from locally cached dependencies; this is **not** P04's complete semver refresh or the historical combined lock. |
| Scratch `cargo +1.98.1 check --offline --workspace --all-targets --exclude openalpaca_gui` | **Exit 101:** two E0277 errors in new memory tests using digest `LowerHex`. |
| Same scratch check, now locked, after adapting only those two missed formatting expressions | **Exit 0**, with the known `proc-macro-error2` future-incompatibility warning. No original source was edited. This is compile evidence for the adjusted scratch graph, not full-wave acceptance. |
| Synthetic CORS responses under tower-http 0.6.11 and 0.7.1 | Both disprove P24's proposed header expectations; details below. |
| Seven current tracked shipped TOML inputs, old/new parser and editor comparisons | Parsed/serialized TOML compares equally. All 42 editor-mutation outputs match after removing a harness-only diagnostic; four existing round-trip failures occur in both versions. This does not cover owner files or typed loaders. |
| Owned, local rmcp 3.4.1 subprocess-drop probe | Direct child dies; forked descendant survives. The owned descendant was explicitly terminated afterward. This characterizes transport drop, not daemon-level disable. |
| Source/diff inspection across the 11 research groups | Completed. Original dated research results remain historical results. |

All build/probe work used temporary paths outside the repository. No daemon/app was launched against the owner's store; no owner configuration, credential store or external service was accessed. No working-checkout upgrade, commit, push or original-document edit was performed. There was **no fresh full Rust/GUI test run, package build, cross-platform CI run, release/advisory inventory, or real-data rehearsal**.

## Findings

### 1. P1 — P10's SHA migration misses two current compile failures

[P10's inventory](/Users/tangyunpei/dev/OpenAlpaca/tasks/deps-upgrade/plan.md:1087) names ten formatting rewrites, including two now-retired storage test helpers. Two new sites replace them, so the total count is coincidentally still ten:

- [memory_tools/tests.rs:1548](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_core/src/tools/builtins/memory_tools/tests.rs:1548)
- [memory_tools/tests.rs:3228](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_core/src/tools/builtins/memory_tools/tests.rs:3228)

Both use `format!("{:x}", sha2::Sha256::digest(...))`. The archived patch does not rewrite them. The isolated compilation reproduced E0277 at exactly these sites under sha2 0.11. The old schema-42 fixture hash and README digest hunks no longer apply; the surviving evaluation helper is now at [eval.rs:445](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_core/tests/common/eval.rs:445).

**Required delta:** replace retired hunks with the two new test adaptations, rescan digest formatting by call site, and keep the `--all-targets` gate. Add independent literal pins for the current project-UUID partition key and project-folder path-hash suffix. Keep the AES fixture, zero-padded hex rule and warning against digest `Debug` formatting.

### 2. P1 — E8 and the SQLite proof describe a retired storage architecture

The plan's SQLite discussion (`plan.md:245–260`, P06 at `1055–1064`) relies on database schema 45 and schema-42 tests. Current [migrations/mod.rs:16](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_storage/src/migrations/mod.rs:16) registers baseline 1, working-state migration 2, and project-store retirement migration 3. The current cache is [schema 6](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_storage/src/memory_index/mod.rs:90), with **three** FTS tables.

E8 (`plan.md:1471–1477`) says project partitions hash a canonical directory and must be scanned at their real paths. Current [project_key](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_storage/src/memory_files/mod.rs:158) hashes the canonical **project UUID**. Project content is under home `projects/<name>/`; its snapshots bind the ID, allowing the copied project stores to be tested inside the copied home store. The in-place scan requirement is obsolete.

There are two further weaknesses in E8's comparison:

- [Home snapshots still bind an absolute root](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_storage/src/memory_files/snapshot.rs:265). Copying the home store to another root can make both builds report the same copy-induced quarantine. Equal reports then do not prove normal snapshot readability.
- [Local doctor opens the ordinary database](/Users/tangyunpei/dev/OpenAlpaca/apps/openalpaca/src/commands/memory.rs:675). On a schema-1/2 copy, that can run existing migrations before E8 captures row counts. [Migration 3](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_storage/src/migrations/003_no_project_store_rows.sql:12) deletes retired in-project asset rows and cascades their links. This remains isolated to the copy, but can hide architecture changes in the proposed before/after dependency comparison.

The preserved SQLite probe wraps **old application source** (main DB 45/cache 3) and checks only two FTS tables. Its `MemoryIndex::open` can recreate a current-version copied cache, so it cannot establish unchanged current-cache reads.

**Required delta:** build old/new dependency probes from the **same selected current application commit**; vary the dependency versions only. Capture the untouched copy's schema and row counts through a raw read-only connection before any application open. Record existing schema upgrades separately. Handle home snapshot relocation explicitly on a disposable fixture/copy; never normalize unknown state with `doctor --repair`. Test UUID-bound projects entirely under the copy, all three FTS tables, clean/WAL reads, current searches, hashes and rollback. Define `<pre-wave>` as today's chosen branch point, not the original research commit.

### 3. P1 — The archived recovery tooling is incomplete and does not stop on failed checks

The plan relies on the preserved replay driver to re-cut patches when G-base fails (`plan.md:915–916`, Appendix A). Inspection found:

- `tools/replay/replay.sh:6–11`, `c.sh`, and helpers hard-code the removed original `SP` layout.
- `replay.sh:54–55` needs `int/frag/kat-llm.rs` and `kat-storage.rs`; neither fragment is packaged. `final-files.txt` is also missing. The pin tests survive in the commit patches, but that does not make this driver runnable.
- `c.sh` records Cargo's status then ends with a successful `echo`. `replay.sh:19–36` logs errors without testing them or stopping. It can continue through failed gates and print `REPLAY DONE`.
- Both preserved SQLite storage copies omit `src/database/` and `database.rs`, while `lib.rs:11` declares that module. They cannot rebuild as packaged.

**Required delta:** leave the archive intact as evidence. Create a separate, portable replay/probe bundle with explicit evidence/export paths, complete fixture inputs, resource preflight checks, explicit store/config/home isolation, and propagated nonzero exits. Replace the stale lock/test hunks; generate the current locks in sequence. Treat the archived 28-step replay as historical reference—it also omits several rev-2 additions and owner-dependent commits.

### 4. P2 — P24 specifies an impossible CORS baseline test

`plan.md:1226` demands allow-origin, allow-headers, allow-methods and expose-headers on both a GET and a preflight. A synthetic offline probe measured this on **both** tower-http 0.6.11 and 0.7.1:

| Response | Headers emitted with the application's permissive configuration |
|---|---|
| GET with Origin | allow-origin, expose-headers |
| OPTIONS preflight | allow-origin, allow-headers, allow-methods |

The proposed test would fail before the bump and after it.

**Required delta:** assert the appropriate header set for each response class. Retain the deliberate decision not to require `Vary`; keep actual packaged ETag/If-Match checks. This correction is to the planned test, not the current CORS implementation.

### 5. P2 — P22's descendant-cleanup gate needs a local fixture and a repair path

P22 (`plan.md:1213–1217`) explicitly requires disabled MCP descendants to be gone, but its evidence covers the direct child only. Current [StdioTransport](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_mcp/src/transport/stdio.rs:79) passes a raw Tokio command into `TokioChildProcess`. In rmcp 3.4.1/process-wrap 10, this path does not automatically register process-group/job-object wrappers.

A bounded local probe of that upgraded transport measured `root_alive=false` and `descendant_alive=true` after drop. The probe's owned descendant was terminated and verified absent. This supports a concrete lifecycle gap; it does **not** prove the complete daemon disable route fails, and it does not establish a newly introduced regression relative to rmcp 0.16.

**Required delta:** add a deterministic forking-server fixture through the actual disable/re-enable path on macOS/Linux, with Windows job-tree coverage where available. Specify a separate lifecycle fix or a fallback/hold if it fails. Keep the owner-server protocol round trip, but do not depend on downloading `npx` content to establish the basic process-tree invariant.

### 6. P2 — GUI and acceptance baselines need recapture on current source

P31 (`plan.md:1293`) names 97 test files/1,165 tests and a historical CSS filename; P33 compares against historical dist hashes. Current source has **119 GUI test files**—a file count, not a passing test result—and substantial changes to approval state, project registration/re-point/deletion, working state, uploads and memory views. G-base (`plan.md:911–916`) currently omits the frontend baseline.

**Required delta:** capture both Rust and all four frontend gates on the exact pre-wave commit, then compare the upgraded build with that baseline under the selected Bun/Node versions. Compare freshly produced CSS/P32/P33 outputs rather than fixed historical filenames. Extend E4/E5 to current project lifecycle, approval re-entry, uploads/artifact Open/Reveal and memory flows; keep packaged WKWebView checks separate from compiler/unit-test evidence. Recompute lint/fmt/test counts rather than using September's counts as literal acceptance criteria.

### 7. P2 — Crypto validation must include the new installed-key and child-environment contract

The plan (`plan.md:208`) says `OPENALPACA_MASTER_KEY` overrides the file. Current daemon boot instead [loads the stored key, installs it once and clears the inherited variable](/Users/tangyunpei/dev/OpenAlpaca/apps/openalpacad/src/main.rs:143). Library reads distinguish installed process key, environment and supplied file location. The AES construction and persisted byte format remain the researched construction; inspection does not show the archived migration undoing this new contract.

**Required delta:** distinguish daemon, library and CLI precedence. Require the current [master-key child-environment integration test](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_llm/tests/master_key_leaves_the_environment.rs:41), CLI state-key tests and no-mint read-only tests after P11. Keep old/new ciphertext fixtures as byte-format evidence; the archived compatibility modules do not cover the newer boot/process contract.

### 8. P2 — Complete the packaging, pairing and cleanup guards

The main isolation rules remain valuable, including disabled credential discovery, explicit store/config roots, a scratch working directory, shell launch and a throwaway app identifier. Three execution gaps remain:

| Gap | Required delta |
|---|---|
| P04 regenerates the lock with unrestricted `cargo update`, while Tauri Rust constraints are broad and JS pins exact. The plan documents that Bun bypasses the CLI's pairing detection. | Check resolved `tauri`/`@tauri-apps/api` and dialog-plugin major.minor pairs **at P04**, not only future Dependabot PRs. Hold researched minors or research/verify the matching pair together. This is a conditional gap; this audit did not check whether a new minor exists. |
| G-ident unconditionally requires rmcp 3.4.1/SQLite 3.53.2 even though the plan permits newer patches and deferring rmcp. Tauri also copies the staged sidecar into the target directory. | Derive marker expectations from the actual lock/DU choices. Save the freshly built daemon checksum **before** bundling; require staged and bundled copies to match it. Keep sidecar removal and separate GUI target directories. |
| Common cleanup (`plan.md:951`) deletes the temporary store; E5 first stops then starts the daemon. The current [launcher deliberately detaches the daemon](/Users/tangyunpei/dev/OpenAlpaca/apps/openalpaca-gui/src-tauri/src/lib.rs:346). | Stop only the recorded temporary daemon, wait for exit/lock release, then remove its store/app data. Closing the window does not complete cleanup. |

### 9. P2 — E6's comparison tools need equivalent code and enforced read-only behavior

E6 (`plan.md:1446–1456`) promises a read-only, index/equality-only check. Inspection of the extracted tools found two issues:

- The archived toml_edit 0.25.15 tool includes an additional `DIFF` diagnostic absent from the 0.25.13 tool and their purported common source. Raw stdout comparisons can therefore report a harness difference as a dependency difference. On seven current tracked TOML inputs, the 42 mutation scenarios have identical outputs after removing only those diagnostic lines. Both versions also report the same four empty-table round-trip failures. Equality between versions must be reported separately from mutation correctness.
- `tools/yamlcmp/src/main.rs:130–168` has a serializer branch that writes `render-*.yaml` beside its corpus for matching `shipped/skill-` paths. Its walker accepts every `.md`. Removing value prints alone leaves a write-capable tool, contrary to the intended real-file gate. The unmodified tool was not run on owner files.

**Required delta:** build both editor variants from one checksum-verified, sanitized source. Remove the YAML serializer/write branch from the real-file comparator, inventory actual skill files explicitly, and verify both inputs and adjacent directories remain unchanged on a synthetic path that triggers the old branch. Report parser failures, cross-version differences and baseline round-trip defects separately. Seven shipped TOML files passed the fresh parser differential; this does not certify the owner's files, current typed loaders or successor-YAML compatibility.

## Other corrections to carry into a separate delta checklist

- P26 (`plan.md:1238`) expects the same shutdown phrase from two CLI commands. [Tail](/Users/tangyunpei/dev/OpenAlpaca/apps/openalpaca/src/commands/tail.rs:193) prints `Connection closed by server`; [confirmations watch](/Users/tangyunpei/dev/OpenAlpaca/apps/openalpaca/src/commands/confirmations.rs:558) now prints `The daemon closed the event stream.` Test graceful closure with the correct per-command wording.
- P28 (`plan.md:1258`) expects an inverse `cargo tree -i native-tls` selector to succeed with empty output when that package is absent. A synthetic absence check exits 101 for the unmatched package selector. Use a successful metadata/full-tree absence check and keep graph absence separate from live TLS/bot behavior.
- DU-1's statements that L8 and slice-4 packaged verification are unfinished are superseded by [the final L8 report](/Users/tangyunpei/dev/OpenAlpaca/tasks/memory-redesign/10-learning-evaluation.md:3) and [the packaged evaluation record](/Users/tangyunpei/dev/OpenAlpaca/tasks/memory-redesign/08-evaluation-report.md:82). Those results have their own commits and limits; they do not validate the dependency wave or today's GUI. The branch remains `feat/markdown-memory`; this audit did not verify a remote merge.
- Rust/JS latest-version claims, action SHAs, advisories, Dependabot's bundled Bun and runner-rollout dates are **September research facts**. Refresh them when implementing. This audit intentionally makes no claim about today's latest releases or advisory status.
- P33's plugin-react-6 choice depends on Vite 8. If P32 is declined, P33 must be declined or retargeted too.
- P38 and any task/vault updates are future implementation work. This audit does not authorize or perform them.

## Coverage of the 11 reports

| Group / plan scope | What remains useful | What must be refreshed |
|---|---|---|
| Toolchain, P03/P04/P36 | Toolchain/MSRV move together; avoid blanket clippy fixes; lock refresh precedes major changes. | Current comparative compilation/tests/lints/fmt, lock graph and Tauri pairing. |
| Secrets, P05/P09–P12 | Exact AES/SHA/base64 compatibility research; keyring disposable-store gate. | Current hash sites/identity pins and installed-key/child-environment contract. No keychain round trip performed. |
| Storage, P06–P08 | SQLite library/WAL compatibility research; lru/sysinfo API findings and platform limits. | DB3/cache6, three FTS tables, UUID partitions, current migration/search/rollback fixtures. |
| Documents, P14–P18 | Extractor and both MIME-validation paths are unchanged from the research base. PDF/ZIP/sheet/sniffing decisions remain relevant. | Characterization tests still need to be written; retain Form-XObject, encrypted/bzip2, separator/C0, text/audio/heix and malicious-PDF cases. |
| MCP, P19–P23 | Staged constructor/model migration and protocol/real-server checks. Archived source patch still applies. | Current full suites, deterministic descendant lifecycle gate and explicit failed-gate remedy. |
| Web/CLI, P24–P28 | Library/API research, TLS and real-terminal checks. Web patch still applies. | Correct CORS test; current streaming/confirmation flows and shutdown messages. |
| Config, P29/P30/E6 | Compatibility options; seven tracked shipped TOML files freshly compare equally. | Equivalent sanitized editor tools, YAML read-only enforcement, current schema/frontmatter corpus, selected parser, typed-loader behavior and all resolved config files. Owner files were not read. |
| GUI/JS/Bun/Tauri, P31–P33 | Selected migration research and v1/v2 lock decision structure. GUI patch applies by context. | Current four gates, freshly built output comparison, current screens, selected Bun and native package proof. |
| CI/supply chain, P01/P02/P13/P34–P40 | SHA pins, locked/frozen installs, platform/advisory/maintenance structure. CI patch applies by context. | Final rev-2 workflows, real runner jobs, current advisories/updater support and variant-aware identity gate. |
| Verify data safety, E6–E8 | Separation of measured library compatibility from owner-data/keychain checks. | Current-source probes and revised copy/doctor/migration/root-binding procedure. |
| Integration / all phases | Ordering constraints and historical 28-step replay/logs. | Portable complete driver, fail-on-error behavior, rebased source/lock, current Rust+GUI baseline and final acceptance. |

A clean patch-context check is not proof of correct behavior. The document/MIME, CORS policy, keyring service/account naming, config/YAML and MCP migration research remains useful within its stated versions and evidence boundaries.

## Proposed delta work, without editing the original plan

1. Freeze the actual upgrade branch point and selected DU/version choices. Record the original research as historical evidence; keep this audit separate.
2. Produce a portable current-source replay/probe package; replace obsolete hunks and the two missed SHA test sites; generate locks from the current graph.
3. Add/refresh P05, P14 and corrected P24 characterization gates before their bumps, plus the deterministic MCP process-tree gate and current crypto-contract coverage.
4. Establish pre-wave Rust and GUI baselines. Retain the broad order: CI/toolchain → lock refresh → pins → library/API updates → GUI → enforcement/close-out.
5. Run the current full suites and platform checks; separately perform current-source SQLite/WAL/FTS rollback, package identity and current UI-flow checks.
6. Before any real-store upgrade, perform revised E6/DU-10/E8 under their required privacy/isolation rules. Keep architecture migrations and dependency changes distinguishable.

The audit is complete. Implementation and owner-data gates are intentionally outstanding because this request was to audit while preserving the originals.

## Audit evidence

Evidence was unpacked outside the repository at `/private/tmp/openalpaca-deps-audit-f4ydlok1/evidence`. The audit folder also retains:

- [Original-file SHA-256 manifest](/private/tmp/openalpaca-deps-audit-f4ydlok1/original-sha256.json) and [preservation verification](/private/tmp/openalpaca-deps-audit-f4ydlok1/preservation-verification.json).
- [Current branch/overlap inventory](/private/tmp/openalpaca-deps-audit-f4ydlok1/drift.json) and [combined patch dry-run result](/private/tmp/openalpaca-deps-audit-f4ydlok1/combined-apply-check.txt).
- [First scratch compile](/private/tmp/openalpaca-deps-audit-f4ydlok1/scratch-upgrade-check.log) and [adapted scratch compile](/private/tmp/openalpaca-deps-audit-f4ydlok1/scratch-upgrade-recheck.log).
- [CORS probe](/private/tmp/openalpaca-deps-audit-f4ydlok1/cors-probe.log) and [MCP descendant probe](/private/tmp/openalpaca-deps-audit-f4ydlok1/descendant-probe.log).
- [Current shipped TOML comparison](/private/tmp/openalpaca-deps-audit-f4ydlok1/current-shipped-toml-comparison.log), [old editor output](/private/tmp/openalpaca-deps-audit-f4ydlok1/current-tedit-13.log), and [new editor output](/private/tmp/openalpaca-deps-audit-f4ydlok1/current-tedit-15.log).
- Detailed scoped audits: [storage/secrets/documents](/private/tmp/openalpaca-deps-audit-f4ydlok1/storage-secrets-documents-audit.md), [config/MCP/web](/private/tmp/openalpaca-deps-audit-f4ydlok1/config-mcp-web-audit.md), and [toolchain/CI/GUI](/private/tmp/openalpaca-deps-audit-f4ydlok1/toolchain-ci-gui-audit.md).

Temporary-path evidence should be copied to a durable location if long-term retention is needed. The original preserved evidence remains in [evidence.tar.gz](/Users/tangyunpei/dev/OpenAlpaca/tasks/deps-upgrade/evidence.tar.gz).
