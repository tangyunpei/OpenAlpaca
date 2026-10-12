# Dependency upgrade plan audit 2026 10 09

**Rev 2.1 remains useful research, but its preserved patches and acceptance gates are not ready to execute on the current codebase.** Rebuild the wave from the current application commit, update the storage rehearsal and repair the verification tools before starting the upgrades.

This audit covers the plan, all 11 group reports, the extracted evidence, and application commit `d484a56a92aac4d7c5e78c29dd25e8761c28b60d` on `feat/markdown-memory`. The existing [October 8 audit](/Users/tangyunpei/dev/OpenAlpaca/tasks/deps-upgrade-audit-2026-10-08.md) targets the same commit. Its major findings still hold; this companion supplies fresh checks, corrections and additional gaps. The original plan, reports, archive and October 8 audit remain unchanged.

The application is **471 commits and 538 changed tracked files** past rev 2's last checked tip, `908050f1`. Seven of the combined patch's 37 paths overlap those changes. The dependency wave itself has not landed: Rust remains 1.93.0, rusqlite 0.38.0, sha2 0.10.9, aes-gcm 0.10.3, keyring 3.6.3, rmcp 0.16.0 and the frontend remains on Vite 7/Vitest 4.

## Checks and evidence boundaries

| Check | Result |
|---|---|
| Combined patch against current checkout, applicability only | Fails in `Cargo.lock`, `storage/src/database/tests.rs` and `storage/src/store/tests.rs`. No patch applied. |
| Documents, MCP, web and config patches, excluding their lock hunks | Each passes an independent applicability check. This does not establish compilation or behavior. |
| Current AES source patch | Applies to the encryption module/tests and preserves the installed process-key implementation. |
| Current frontend in an export outside the repository | Type check, tests, formatting and production build all pass. **119 test files, 1,453 tests.** Bun 1.3.13, Node 24.15.0; existing installed modules copied without an install. |
| CLI plus daemon versus daemon-only dependency features | The joint build enables reqwest `multipart`; the daemon-only build does not. This affects E5's binary-identity procedure. |
| Source and archived-tool review | Current storage identities, schemas, crypto contracts, CORS behavior, comparator writes and replay failure handling independently checked. |
| Upstream freshness spot checks | Rust 1.99.0 is released; Dependabot main now specifies Bun 1.4.2 and permits lockfile versions through 3. Hosted rollout not verified. |

The October 8 audit's isolated Rust compilation, CORS probe and subprocess probe remain **prior-run evidence**, not newly executed tests. This audit did not run the upgraded full Rust/frontend suites, a native package, cross-platform CI, a fresh advisory census, a frozen-lock reinstall or an owner-data/keychain rehearsal. No daemon or app was launched.

The archive was extracted to [the audit scratch directory](/private/tmp/openalpaca-deps-audit-_gj26hpv), outside the repository. It contains the new check logs and scope notes.

## Findings that block execution

### P1 The patch inventory and replay fallback need rebuilding

[P10](/Users/tangyunpei/dev/OpenAlpaca/tasks/deps-upgrade/plan.md:1087) still names retired storage test expressions. Two current digest-formatting expressions at [memory_tools/tests.rs:1548](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_core/src/tools/builtins/memory_tools/tests.rs:1548) and [line 3228](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_core/src/tools/builtins/memory_tools/tests.rs:3228) are absent from the patch. They use LowerHex formatting that sha2 0.11 no longer provides. The previous audit reproduced E0277 at both; today's source still contains both.

Re-cut source changes against the selected current commit, adapt those expressions, regenerate locks through Cargo in sequence, and retain the workspace `--all-targets` compilation gate. Do not restore retired fixtures merely to make historical hunks apply.

The [G-base fallback](/Users/tangyunpei/dev/OpenAlpaca/tasks/deps-upgrade/plan.md:911) cannot rely on the archived driver unchanged. Its scripts hard-code the removed scratch layout, reference missing fixture fragments and `final-files.txt`, and can continue after failed Cargo gates: `c.sh` records the exit code but returns its final successful echo; `replay.sh` logs check failures without stopping. The preserved 28-step replay also does not implement every rev-2 addition.

**Required:** a separate portable replay bundle with complete inputs, current patches and explicit propagation of every failed check. Keep the archive as historical evidence.

**Correction to the October 8 audit:** project-folder hash suffixes already have independent literal pins in [store/projects/tests.rs:99](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_storage/src/store/projects/tests.rs:99), including normalization and long-path cases. Retain and run them rather than adding duplicates. The project-UUID partition assertion still derives its expected value from the same hashing helper at [memory_files/tests.rs:82](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_storage/src/memory_files/tests.rs:82); an independent literal is useful there.

### P1 E8 no longer represents the storage being upgraded

[E8](/Users/tangyunpei/dev/OpenAlpaca/tasks/deps-upgrade/plan.md:1466) assumes path-derived project keys and a necessary scan of real project partitions in place. Current projects live under the home store and [partition keys hash the project UUID](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_storage/src/memory_files/mod.rs:158). Current application migrations are [1, 2 and 3](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_storage/src/migrations/mod.rs:23); the memory cache is [version 6](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_storage/src/memory_index/mod.rs:90), with three FTS tables.

Rebuild both comparison programs from **the same current application commit**, changing dependency versions only. Preserve project UUIDs and test their copied content under the copied home store. Explicitly account for home snapshots still binding an [absolute root](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_storage/src/memory_files/snapshot.rs:265): identical copy-induced quarantine in both builds does not prove healthy snapshot reads.

Before either application opens the copy, capture schema versions and row counts through a raw read-only connection. [Local doctor opens the ordinary application database](/Users/tangyunpei/dev/OpenAlpaca/apps/openalpaca/src/commands/memory.rs:672), which can migrate a schema-1/2 copy before E8 records its baseline. [Migration 3](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_storage/src/migrations/003_no_project_store_rows.sql:12) deletes retired project-store rows. Keep existing architecture migrations distinct from dependency compatibility.

**Additional gap:** equal doctor reports do not establish complete coverage. [Doctor](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_storage/src/memory_files/doctor.rs:147) discovers unnamed project partitions through snapshots, skips project IDs without registered folders, and continues with no listed projects after a listing error. A [current test](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_storage/src/memory_files/tests.rs:4194) explicitly preserves one such omission.

Add an independent inventory of project records, owner directories and control snapshots. Account for every expected partition and any legacy/id-less snapshot, exercise registered projects explicitly, and fail the rehearsal on listing warnings or unexplained omissions. Keep the existing prohibition on starting a daemon against copied owner data; relocation does not establish safe runtime isolation.

### P1 The preserved comparison tools cannot be used as acceptance tools unchanged

The SQLite probe's two storage copies are incomplete: each declares a database module absent from the archive. They also embed the old application schema/cache and only check two FTS tables. Opening today's cache through that old application can recreate it. Its FTS error path prints failures without making them fail the process.

[E6](/Users/tangyunpei/dev/OpenAlpaca/tasks/deps-upgrade/plan.md:1446) likewise needs more than suppressed logging:

- The archived YAML comparator can write `render-*.yaml` for matching corpus paths and recursively accepts all Markdown files. A real-file comparator must remove this serializer branch and explicitly inventory skill inputs.
- The new-version TOML editor comparator has an extra diagnostic absent from the old-version comparator, so raw output comparison mixes dependency differences with harness differences.
- Parser equality does not validate the current typed configuration. Include current `carry_over_turns` behavior and the retirement of `episode_max_per_day` and `compaction_model`.

Build each old/new pair from one sanitized source, differing only in dependencies. Verify that synthetic inputs and adjacent directories remain unchanged, propagate errors, and distinguish shared baseline defects from differences between dependency versions. Do this before any owner-file gate.

## Verification corrections

| Priority and plan scope | Correction required |
|---|---|
| **P2 E5 and G-ident** | The sidecar build is not necessarily a no-op after a joint CLI/daemon build. [CLI features](/Users/tangyunpei/dev/OpenAlpaca/apps/openalpaca/Cargo.toml:20) differ from the [daemon-only sidecar invocation](/Users/tangyunpei/dev/OpenAlpaca/apps/openalpaca-gui/scripts/prepare-sidecar.ts:59). Current feature graphs confirm this; the [October 2 package log](/Users/tangyunpei/dev/OpenAlpaca/scripts/eval/memory/reports/slice4-packaged-2026-10-02/d1-03-tauri-build.txt:4) independently records a real rebuild. Finish the exact daemon-only release build before capturing an immutable expected checksum, then compare staged and bundled copies to it. Alternatively use one consistent feature/build invocation throughout. This refines the October 8 audit's checksum recommendation. Derive version markers from the chosen lock and DU decisions. |
| **P2 P04 and Tauri pairing** | An unrestricted lock refresh can move broad Rust Tauri requirements independently of exact JS pins. Check resolved major/minor pairs at P04; hold researched minors or upgrade the paired packages together. Preserve a separate GUI build target directory so its build script cannot overwrite the daemon used for comparison. |
| **P2 E4/E5 and GUI baselines** | Replace September's 97 files/1,165 tests and CSS filename with a baseline captured at the selected branch point. Today's four frontend gates pass with 119 files/1,453 tests. Add current project registration/repoint/deletion, approval re-entry, working state, upload/artifact Open/Reveal and memory flows to packaged acceptance. Unit tests do not validate WKWebView. Stop the recorded temporary detached daemon and wait for exit/lock release before deleting its store. |
| **P2 P11 crypto contract** | The plan's environment-over-file description is stale for daemon boot. [Boot installs the file key and clears the inherited environment variable](/Users/tangyunpei/dev/OpenAlpaca/apps/openalpacad/src/main.rs:143). Retain the AES byte-format fixtures and also run the [child-environment integration test](/Users/tangyunpei/dev/OpenAlpaca/crates/openalpaca_llm/tests/master_key_leaves_the_environment.rs:41), CLI key-loading and no-mint tests. The archived crypto comparison alone does not cover this process contract. |
| **P2 P24 CORS** | [The proposed test](/Users/tangyunpei/dev/OpenAlpaca/tasks/deps-upgrade/plan.md:1226) demands all four CORS headers on both GET and preflight. Both reviewed dependency versions emit allow-methods/allow-headers for OPTIONS and expose-headers for non-OPTIONS. Split assertions by response class. This corrects a planned test, not an application regression. |
| **P2 P22 MCP cleanup** | Keep the descendant-cleanup requirement, but add a deterministic forking-server fixture through the actual disable/re-enable path and a repair/hold outcome. The raw command path does not automatically register process-tree wrappers. Prior transport-drop evidence does not prove daemon-level behavior or a new regression relative to rmcp 0.16. |
| **P2 E3 completion policy** | [Empty outdated/update output](/Users/tangyunpei/dev/OpenAlpaca/tasks/deps-upgrade/plan.md:1423) conflicts with the [seven-day freshness rule and allowed major holds](/Users/tangyunpei/dev/OpenAlpaca/tasks/deps-upgrade/plan.md:908), including optional P32. Require that every difference from the approved version set is explained; do not demand upgrades beyond the chosen scope to turn this gate green. |
| **P3 P26/P28 command assertions** | Tail and confirmations now use different graceful-close messages. An inverse Cargo-tree selector for an absent package fails with a nonzero status; use a successful graph inventory for the no-native-TLS assertion. Keep graph evidence separate from live TLS/bot behavior. |

The document extraction/MIME implementations, MCP crate and skill parser remain unchanged from the research base in the reviewed paths. Their API and format research still applies; it needs current tests and the corrected gates above, not wholesale replacement.

## Freshness and sequencing decisions

**DU-13 needs an updated recommendation.** Its premise that Dependabot main bundles Bun 1.3.14 and cannot accept v2 is obsolete. The current [updater Dockerfile](https://raw.githubusercontent.com/dependabot/dependabot-core/main/bun/Dockerfile) specifies **1.4.2**, and the current [version manager](https://raw.githubusercontent.com/dependabot/dependabot-core/main/bun/lib/dependabot/bun/bun_package_manager.rb) permits lockfile versions through **3**. This meets the source-level trigger already written at [plan line 821](/Users/tangyunpei/dev/OpenAlpaca/tasks/deps-upgrade/plan.md:821). Confirm the actual hosted updater/job before asserting rollout, then generate and validate the lock with the chosen pinned Bun instead of automatically retaining a two-version authoring workflow.

**DU-2 is now a real choice.** [Rust 1.99.0 was released October 1](https://github.com/rust-lang/rust/releases/tag/1.99.0), so it clears a seven-day age threshold on October 9. Keeping researched 1.98.1 remains a supported scope choice; moving to 1.99 requires fresh compiler/lint/fmt comparisons. This spot check does not replace a complete release/advisory inventory at implementation time.

The old prerequisite statements about unfinished L8 and slice-4 packaging are superseded by the [L8 record](/Users/tangyunpei/dev/OpenAlpaca/tasks/memory-redesign/10-learning-evaluation.md:3) and [packaged evaluation](/Users/tangyunpei/dev/OpenAlpaca/tasks/memory-redesign/08-evaluation-report.md:82). Those records certify their own commits and limits, not this dependency wave or the current GUI. Remote merge status was not checked.

## Recommended execution order after the audit

1. Select and record the exact current application commit and approved version/hold matrix. Define pre-wave rollback builds from that same application commit.
2. Capture current Rust and frontend baselines; rebuild portable replay and comparison tools. Replace stale patch/test hunks and establish data-format pins before version changes.
3. Keep the broad original phase order: CI/toolchain, lock refresh, format pins, storage/crypto, document handling, MCP/web/config, GUI, final checks. Reapply the owner choices to the refreshed plan rather than assuming the archived 28 commits implement all 43 entries.
4. Require corrected storage-copy, process-lifecycle, packaged identity and current GUI acceptance before any real-store upgrade.

No source or dependency changes were made. The sole repository addition is this companion report; original documents remain intact.

Detailed fresh evidence: [drift summary](/private/tmp/openalpaca-deps-audit-_gj26hpv/drift.json), [frontend results](/private/tmp/openalpaca-deps-audit-_gj26hpv/gui-baseline-results.json), [storage and secrets notes](/private/tmp/openalpaca-deps-audit-_gj26hpv/storage-safety-notes.md), [documents MCP web and config notes](/private/tmp/openalpaca-deps-audit-_gj26hpv/api-deps-notes.md), [toolchain CI and GUI notes](/private/tmp/openalpaca-deps-audit-_gj26hpv/toolchain-ci-gui-notes.md). Temporary evidence paths may be cleaned later; the conclusions and source anchors above are retained in this report.

