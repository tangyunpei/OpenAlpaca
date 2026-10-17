# Toolchain, dependency and modernization plan — review (2026-10-17)

**Status: rev 1, 2026-10-17.** Reviewed: `tasks/deps-upgrade/plan-rev3.md` (rev 3.12: Part I, Part II, Appendix B, DU-1…DU-33), `tasks/deps-upgrade/modernization-plan.md` (rev 2.1: Part III), `tasks/deps-upgrade/reports/2026-10-12-no-compromise-replan.md`, the G-base driver (`tasks/deps-upgrade/tools/gbase/`: `gbase.sh`, `README.md`, `lockdiff.py`, `lockdates.py`), the E6 comparators (`tasks/deps-upgrade/tools/e6/`), `tasks/deps-upgrade/baselines/2026-10-12/README.md` (Q00), `tasks/deps-upgrade/README.md`, `tasks/pending-decisions.md` §4.3, `tasks/todo.md` Chunk 25 and `CLAUDE.md`. All of it at HEAD `3598a6fb` of `chore/toolchain-and-deps` (never pushed), plus the pre-written commits pinned at `refs/prep/deps-wave-2026-10-12` (tip `7e702582`, base `6992a402`).

## 1. What was reviewed and how

**Method.** The review was split into seven slices:

- **A1:** Part I P01–P19b and the actions before the first commit.
- **A2:** P20–P42, the E1–E8 gates, §8 and §9.
- **B:** upstream freshness, advisories and the cut timing.
- **C:** Part II.
- **D:** Part III.
- **E:** consistency across the documents, the decisions and the no-compromise rule.
- **F:** safety, data integrity and the verification tooling.

Each slice's findings went to an adversarial verifier. The verifier re-read the cited lines or re-ran the evidence (registry reads, offline `cargo` runs in scratch exports, toy reproductions), then gave a verdict and a corrected severity. A completeness critic then went over the whole set and added nine self-verified findings, G-01…G-09.

**Counts.**

- The slices raised 123 findings. 122 survived: 71 were confirmed, 51 were confirmed only in part (real but narrower; the verifier's reading is used below), and one (slice A1's thirteenth) was refuted.
- With the critic's nine, that makes **131 findings**. By verified severity: **0 critical, 11 high, 45 medium, 75 low**.
- Several slices found the same defects independently. After merging those duplicates there are **6 must-fix items, 29 should-fix items and 50 minor rows**. Every source id is kept with the item it was merged into.

**Scope of the review session.** Nothing in the repository was written except this file. No daemon was started, no version was moved, and `~/.openalpaca` was not read. Registry facts come from crates.io, npm, static.rust-lang.org, GitHub and RustSec's advisory-db, read on 2026-10-16/17 UTC.

---

## 2. Verdict

The plan is not safe to execute at the cut as written: its rules are sound and what it measured on 2026-10-12 holds, but three upstream facts arrived after rev 3.12 and nothing in the pipeline notices them — Tauri's 2026-10-14 patch train, which P31a's own procedure takes at the recommended 2026-10-17T20:08Z cut; rustls 0.23.46, which fixes four RustSec advisories that a 10-17 cut must keep; and Rust 1.99.1, which fixes an x86_64 miscompilation CI's runners are exposed to. DU-23's 7-day rule is checked by machine only for JS, and G-base, the wave's first gate, labels young Rust releases "expected" and can pass a run in which a test binary crashed. Two verification legs (E5 and G-bundle leg A) have the unsandboxed app spawn a daemon outside the sandbox, and M63, which lands early, would silently defeat every isolation that relies on `CARGO_TARGET_DIR` alone. Before the cut: amend MF-1 to MF-6 (§3), all documentation or tooling changes that move no version and that DU-23 therefore permits now; have the owner answer D1–D3 (§6: the cut instant, rustls, Rust 1.99.1); and put in place §4's before-the-cut items — backups at the right moment, a draft PR so that CI runs at all, a procedure for landing the prep commits, and a checklist regenerated from the current plans.

---

## 3. Must fix before the cut (high)

### MF-1 — The 2026-10-14 Tauri train is taken at the recommended cut, and the driver calls it expected

*Merged from:* A1-01, A2-01, B-01, E-01, with G-03 and G-04.

**What the registries show.** On 2026-10-14, between 18:47 and 19:53Z, Tauri published a whole patch train.

- crates.io:
  - tauri 2.12.4 (18:52:10Z) and tauri-build 2.7.3 (18:51:08Z)
  - tauri-codegen and tauri-macros 2.7.3
  - tauri-runtime and tauri-runtime-wry 2.12.2
  - tauri-utils 2.10.2 (18:47:14Z) and tauri-plugin 2.7.2
  - tauri-plugin-fs 2.7.1 (19:15:53Z) and tauri-plugin-dialog 2.8.3 (19:16:07Z)
- npm:
  - @tauri-apps/api 2.12.4 (18:47:44Z) and plugin-dialog 2.8.3 (19:17:55Z)
  - cli 2.12.3 (19:50:25Z). Its last platform binary, cli-linux-riscv64-gnu, is from 19:52:53Z.

None of these is in DU-23's minute table (`plan-rev3.md:536-553`). None is eligible at "the cut at or after 2026-10-17T20:08Z" (`:553`).

**How P31a takes them.** P31a (`:705`) runs `cargo update -p tauri -p tauri-build -p tauri-plugin-dialog`, then `--precise` on tauri alone.

- The targeted update has to unlock tauri's internals from HEAD's 2.11.x. tauri 2.12.3 requires them by tilde (`tauri-utils ~2.10.1`, `tauri-runtime(-wry) ~2.12.1`, `tauri-macros ~2.7.2`, per the crates.io dependencies API). Every 10-14 release satisfies those ranges, so cargo resolves to the 10-14 releases.
- Slice A1 reproduced this offline on an export of `3598a6fb` with step 1b's manifest edits. tauri moved to 2.12.4. `--precise 2.12.3` then moved only tauri back; tauri-build 2.7.3, codegen and macros 2.7.3, runtime(-wry) 2.12.2 and utils 2.10.2 stayed. The verifier notes that the exact set depends on the local index snapshot; in its run tauri-plugin went to 2.7.1 and tauri-plugin-fs moved too.
- swift-rs 1.0.9 no longer holds the targeted update back. So the rationale for the precise step is stale, both in P31a and in `tasks/deps-upgrade/tools/gbase/gbase.sh:341` ("a targeted update lands on 2.12.2 (swift-rs)").

**How P04 and the driver let them through.**

- P04's "No Tauri step" (`:710`) is wrong under `~2.12`: its refresh takes tauri 2.12.4 again.
- The driver passes this state:
  - Step 1b checks tauri-build by minor only (`expect_lock tauri-build ere_minor 2.7`, `gbase.sh:362-378`).
  - `lockdates.py` is "an annotation of a report, not a gate" (`lockdates.py:16`).
  - The README counts every release dated after 2026-10-10 as expected (`tools/gbase/README.md:83`).
  - `--with-stepbacks` is refused.

**Why stepping back is not simple.** Every 10-14 internal requires `tauri-utils ~2.10.2`. A step-back is therefore an ordered cascade with tauri-utils last, and the wrong order fails ("failed to select a version for the requirement `tauri-utils = \"~2.10.2\"`").

**What is already protected.** bunfig.toml's `minimumReleaseAge = 604800` and `tauri-plugin-dialog = "=2.8.2"` hold the npm releases and dialog. The exposure is Rust-only.

**The plan contradicts itself.** `:553` says "the Tauri set is never held" (DU-26 (a′), `:576`). DU-23 says to step back anything younger than 7 days.

**The cut instant is ambiguous (G-03).** DU-23 (a) (`:531`, "Cut on or after 2026-10-17"), §5.1 item 5 (`:630`) and Chunk 25's heading (`tasks/todo.md:571`) give a date only, and the owner's clock is PDT (UTC−7). A date-only reading in either zone admits releases early. `gbase.sh` has no clock check: `now()` (`:133`) only stamps logs.

**Why it matters.**

- An owner decision (DU-23) is broken silently, and the run of record and every gate pass the result.
- The 10-14 mix — a 2.12.3 core over 2.12.2/2.10.2/2.7.3 internals — was never released together by Tauri and never compiled under G-GUI, whose anchors are on the 2.12.1/2.10.1/2.7.2 internals.

**Amendment.**

1. **The owner chooses the cut (§6 D1).** Do not adopt a date silently.
   - **(a) Cut at or after 2026-10-21T19:53Z (12:53 PDT).** The last of the family on either registry, the npm cli's riscv64 binary, is then 7 days old; the Rust crates alone clear at 19:16:07Z (tauri-plugin-dialog 2.8.3). Under (a):
     - Re-anchor P31a, DU-23's table, `gbase.sh:341-344` (TAURI_PRECISE, DIALOG, JS_API, JS_CLI) and step 1b's reference lock on tauri 2.12.4 / tauri-build 2.7.3 / `tauri-plugin-dialog = "=2.8.3"` ↔ @tauri-apps/api 2.12.4 / plugin-dialog 2.8.3 / cli 2.12.3.
     - Re-measure G-GUI and G-bundle.
     - This takes 2.12.4's fix for a panic in the IPC and asset protocol handlers on an empty URI path.
   - **(b) Keep 20:08Z.**
     - Write P31a's step-backs into its entry: measured on the day, the lock re-read after each `--precise` because the steps cascade, each spelled `name@locked`, tauri-utils last.
     - Teach the driver the same steps.
     - Add tauri → 2.12.3 to P04 and reword "No Tauri step".
     - Do not copy any step-back list from this review; measure it on the day.
2. **Under either option:**
   - Rewrite P31a's `=` rule as "the newest version both registries carry *that is eligible*".
   - Delete the swift-rs rationale, in `:705` and in the comment at `gbase.sh:341`.
   - Make step 1b check every Tauri crate exactly, and check that each one's `created_at` is at or before the cut minus 7 days: tauri, -build, -codegen, -macros, -runtime, -runtime-wry, -utils, -plugin, -plugin-fs and -plugin-dialog.
   - Reword `:553` to "no hold except DU-23's freshness step-back".
3. **Write the cut as an instant with its zone** everywhere it is named: DU-23 (a), §5.1 item 5, Chunk 25 and `tools/gbase/README.md`. For the current recommendation that reads "2026-10-17T20:08Z (13:08 PDT)". Give `gbase.sh` a `--not-before <UTC>` and make it refuse to start before that instant.

### MF-2 — The Rust side of the 7-day rule is not enforced, and the prep commits' locks are not eligible at 20:08Z

*Merged from:* A1-02, B-03, B-04, F-10, with E-01 and G-04.

**What the plan already says.** P04's text (`plan-rev3.md:710`) does say to step back only packages younger than 7 days, reading `cargo update --dry-run --verbose` against DU-23's minute table. The verifiers of A1-02, B-03 and E-01 agree that this text is not wrong. What is missing is enforcement and a current table.

**Where the rule goes unchecked.**

- `lockdates.py` exits 0 regardless (`lockdates.py:82-86`).
- The cut-day run is non-strict (`tools/gbase/README.md:83`).
- G-base step 2 is "a plain refresh, no step-back".
- P10 (`:743`), P12 (`:754`), P16 (`:771`) and Chunk 25's "rebase the prep commits … re-gate each" (`tasks/todo.md:569`) carry no instruction about age.

**What HEAD's lock would need at a 20:08Z cut.** An approximate sweep of HEAD's lock against crates.io finds about 33 entries that would need a `--precise` step-back, the ten Tauri crates among them. The sweep ignores MSRV, feature and target filters, so the count is approximate. Examples:

| Crate | Newest (published) | Approximate eligible target at 20:08Z — measure on the day |
|---|---|---|
| aws-lc-rs | 1.18.2 (10-12T15:05Z) | 1.18.1 |
| cc | 1.8.0 (10-11T14:17Z) | 1.7.0 |
| libc | 0.2.191 (10-16T22:56Z) | 0.2.190 |
| serde_json | 1.0.154 (10-11T15:31Z) | 1.0.152 |
| syn | 3.0.9 (10-12T15:04Z) | 3.0.7 |
| tokio | 1.53.3 (10-13T20:51Z) | 1.53.2 |
| unicode-ident | 1.0.27 (10-12T04:20Z) | 1.0.26 |
| unicode-normalization (on P04's data-path watch-list) | 0.1.26 (10-16T23:28Z) | stays at HEAD's 0.1.25 |
| zstd-sys | 2.1.2 (10-16T16:59Z) | not measured (HEAD holds 2.0.16) |
| zopfli | 0.8.4 (10-11T19:45Z) | 0.8.3 |

**Caret lines resolve fresh when the lock is regenerated.**

- P10's `sha2 = "0.11"` resolves to 0.11.1 (10-12T18:58Z).
- P12's `keyring-core = "1.0"` resolves to 1.0.1 (10-13T00:02Z).

**The prep lock.** The lock at `refs/prep/deps-wave-2026-10-12` holds cc 1.8.0, serde_json 1.0.154, syn 3.0.8, zopfli 0.8.4, unicode-ident 1.0.27 and zip 9.0.2. None of them is eligible at 20:08Z. As integrated, the prep set is eligible only from 2026-10-19T04:20:27Z, when unicode-ident 1.0.27 turns 7 days old.

**P16's floor.** Prep P16 (`5c265232`) pins `zip = { version = "9.0.2", … }` (`refs/prep/deps-wave-2026-10-12:Cargo.toml:164`). That caret floor excludes 9.0.1, the only eligible 9.0.x at 20:08Z; 9.0.2 is eligible only from 10-18T23:49Z, and 9.0.3 and 9.0.4 have since appeared. P16's own text says "<newest 9.0.x eligible at the cut>" (`:771`), which contradicts the pin.

**DU-23 (a) contradicts its own table.** DU-23 (a)'s "take the measured set as the eligible set" (`:531`) is contradicted by rev 3.9's table, which dates serde_json 1.0.154 as eligible on 10-18.

**Mid-wave releases (G-04).** The wave is about 150 commits over several days. More releases become eligible while it runs, yet the driver's pins are literals (`gbase.sh:341`, `:598-616`), and E3 checks against the matrix "as of the cut" only.

**Why it matters.** The owner's rule is broken with every gate green. And the prep figures (5,126/0/17, clippy 0, the fmt counts) describe a lock that no landed commit will reproduce.

**Amendment.**

1. **Give `lockdates.py` a gate mode for cargo** (bunfig.toml already binds JS): `--gate --max-date <UTC>`.
   - It exits non-zero for any (name, version) a commit adds or moves relative to its parent lock whose crates.io `created_at` is later than the max date, unless that entry is listed as a recorded DU-23 (c) waiver.
   - A failed lookup must also fail in gate mode; today it prints "(date unavailable)" and exits 0.
   - It prints the `--precise` sequence, dependents first.
   - Run it in G-base step 2 and in every commit that moves the lock: P31a, P04, P06–P19b, P25–P30, P41 and later.
2. **Refresh the minute table on the cut day** from the registries. Reword DU-23 (a) to "the newest eligible at the cut, checked by the gate". That corrects the wording to match rev 3.9's own table; it is not a new decision.
3. **Extend Chunk 25's "At the cut" item.**
   - Amend P16's floor to the version taken: 9.0.1 at a 20:08Z cut, then `cargo update -p zip@<locked> --precise 9.0.1`.
   - Re-pin each prep lock to eligible versions.
   - Measure each step-back on the day, because they cascade.
4. **Add a freshness sweep before the merge.** Each release that became eligible after the cut gets its own commit with its gate, or a named blocker. Turn the driver's Tauri pair and JS pins into parameters checked against registry dates.

### MF-3 — rustls 0.23.45 carries four advisories published 2026-10-14

*Merged from:* A2-02, B-02, with B-09.

**The advisories.** RustSec commit `caee11f` (2026-10-14) added four advisories against rustls, all patched in rustls ≥ 0.23.46:

- **RUSTSEC-2026-0346:** Debug impls of client session values expose TLS secrets.
- **-0347:** an ECH rejection sends buffered data.
- **-0348:** Brotli certificate decompression can allocate 1 GiB.
- **-0349:** an unauthenticated ServerHello downgrades later connections to classical key exchange. It has no unaffected range.

**The timing.** rustls 0.23.46 was published 2026-10-14T09:54:15Z and is eligible from 2026-10-21T09:54:15Z. At a 20:08Z cut the newest eligible release is 0.23.45, which is P04's target (`plan-rev3.md:135`). HEAD's 0.23.43 is not affected by -0347 (unaffected below 0.23.44), so P04 *introduces* that one.

**Reachability.**

- -0348 is inert: the prep lock's rustls has no brotli dependency.
- -0346 and -0347 are unlikely: there is no ECH and no custom `ClientSessionStore`.
- -0349 applies to any client with resumption on, which is rustls's default — so to every HTTPS client in the daemon.

**What passes and what fails.** P04's own gate names only -0285 (`:714`), so it literally passes. E2's "cargo-audit 0 vulnerabilities" (`:927`) and P34's "advisories ok" (`:865`) fail.

**The conflict with §0.** §0 item 2 says that "no freshness waiting may delay P04, P15 and P16" (`:89`). That conflicts with waiting for 0.23.46.

**The advisory inventory is also stale (B-09).** Against advisory-db `64f92cd`, HEAD's lock now matches 14 RustSec vulnerabilities, not five. The extra nine are:

- quinn-proto 0.11.17: -0338, -0340, -0343. P04 clears these silently, since the prep lock has 0.11.19.
- rustls 0.23.43: -0346, -0348, -0349.
- zip 2.4.2: -0367, -0368, -0369.

unsafe-libyaml also carries RUSTSEC-2024-0449 (unmaintained, imported 2026-10-12). The plan's database snapshot (2026-10-09) predates three imports.

**Amendment.**

1. **The owner chooses (§6 D2):** a DU-23 (c) waiver for rustls 0.23.46, or a cut at or after 2026-10-21T09:54:15Z. MF-1's option (a) covers the second.
2. **If 0.23.45 ships anyway,** write a reasoned temporary ignore into P34's `deny.toml` and the cargo-audit configuration, rather than leaving E2 red or deciding the waiver at the gate.
3. **Update the text:**
   - §1.2 (`:135`).
   - P04's gate, to "RUSTSEC-2026-0285 and -0346…-0349 gone".
   - E2's list of cleared ids.
4. **At the cut,** re-run cargo-audit and cargo-deny on a freshly fetched database. Rewrite §2.11, the bodies of P04, P15, P16, P19 and P30, and E2's id list from that run, naming quinn-proto's three as cleared by P04.

### MF-4 — Starting the daemon from the app during E5, G-bundle leg A and M74's gate spawns it unsandboxed

*Merged from:* A2-04, C-01.

**The launcher arrangement.** Rev 3.8's launcher starts the daemon under launcher (i) and runs the app unsandboxed (`baselines/2026-10-12/README.md:7`). Three checks then have a person stop and start the daemon from the app:

- G-bundle leg A checks "stop and start" (`plan-rev3.md:661`).
- E5 step 5 runs "Models & keys add a key → restart → it loads … Stop daemon then Start daemon; Chat, Memory and Settings" (`:930`, `:931`).
- M74's gate inherits this (`modernization-plan.md:840`).

**What the code does on Start.**

1. `startDaemon` clears the stop intent and reconnects (`apps/openalpaca-gui/src/lib/daemon-control.ts:146-151`).
2. That reaches `ensure_daemon_running` (`src/lib/connection.ts:298`).
3. With no live discovery file, the shell calls `spawn_daemon()` (`src-tauri/src/lib.rs:38-52`).
4. `spawn_daemon()` execs `<exe dir>/openalpacad` with `setsid`, setting only `OPENALPACA_CONFIG_DIR` and the managed-log variable (`:344-366`).

No sandbox profile is applied.

**Consequences.**

- Every check after the first in-app Start — model turns, the memory flows, Stage 2 file writes, all after a key was added — runs on a daemon with full outbound network. That is the breach `tasks/lessons.md:87-89` records.
- The shell-side two-request proof cannot detect it: it proves the shell's sandbox, not a process the shell spawned.
- The README's sentence that "the only process with outbound reach beyond loopback stays confined" is true only until the first in-app Start.
- Before M77, the unsandboxed window can also follow external links.

**Amendment (C-01's corrected form).** Make every in-app Start confined, and keep G-ident intact.

1. **G-bundle leg A** (a debug bundle, before Q04): remove the staged sidecar from a copy of the bundle, and put a wrapper named `openalpacad` first on `PATH`:

   ```
   exec sandbox-exec -f scripts/eval/memory/loopback-only.sb <real daemon> "$@"
   ```

   The debug fallback (`lib.rs:295-299`) then resolves the wrapper.
2. **From Q04 on, and E5's release bundle:**
   - Run G-ident on the untouched bundle first.
   - Then, only in a copy used for the GUI legs, replace `Contents/MacOS/openalpacad` with the wrapper, beside `openalpacad.real`, and hash `.real`.
   - Alternatively, do the Start by hand under launcher (i), so that no run that carries a model ever tests an in-app Start.
3. **After every in-app Start, prove confinement by Q00's method:** `sandbox_check(network-outbound)` on the new daemon pid, a refused public request in its log, and a `ps` check that the running pid is the confined one.
4. **Keep the key-persists check here.** Under leg B (the app itself under `sandbox-exec`) the webview cannot reach its daemon (Q00).
5. **Correct the README sentence.** Before M77, point the Markdown-link checks at loopback URLs.

### MF-5 — M63's build-dir silently defeats every isolation that relies on `CARGO_TARGET_DIR`

*Merged from:* D-01.

**What M63 does and when.** M63 (`modernization-plan.md:651-654`) adds `[build] build-dir = "{cargo-cache-home}/build/{workspace-path-hash}"` to the tracked root `.cargo/config.toml`. It lands early: "after plan-rev3's P03 (its one `cargo clean`) and P31a" (`:951`).

**What the verifier reproduced on cargo 1.99.0.** With a configured build-dir, `CARGO_TARGET_DIR` moves only the final artifacts; the intermediates still go to the build-dir. `cargo clean` removes both (measured).

**Who is affected.** `git archive` exports carry the root config. So after M63, every export built with `CARGO_TARGET_DIR` alone writes a new `<cargo home>/build/<path-hash>` that deleting the scratch tree never removes:

- the G-base driver (`gbase.sh:540-543`, `:770`, `:956-957`, with `CARGO_HOME` set to the real `~/.cargo`);
- plan-rev3's export recipes;
- E6 builds;
- agent scratch builds under the session rule.

**Other effects.**

- DU-M5 (b) itself admits the growth moves "out of sight of `du target`" (`:554`).
- V4's `du -sh target` expectation (`:990`) stops meaning anything.
- For the shell, `CARGO_TARGET_DIR` without `CARGO_BUILD_BUILD_DIR` splits target-dir from build-dir. tauri-build's sidecar copy walks `OUT_DIR` up to `build`, so under `tauri dev` it lands in the build-dir tree.
- M63 already pairs the two variables for the root `cargo check -p openalpaca_gui`. The pairing was understood, but applied to that one case only.

**Why it matters.** It brings back goal (3)'s unbounded growth, inside the owner's `~/.cargo`. The G-base run of record (before the cut) is unaffected, but every later G-base or E6 run leaks gigabytes while M63's listed gate passes.

**Amendment.** This is a tooling and documentation change with no source change, so it does not conflict with §3.5.

1. M63 also makes these set `CARGO_BUILD_BUILD_DIR` beside every `CARGO_TARGET_DIR`:
   - the G-base driver,
   - plan-rev3's export recipes,
   - CLAUDE.md's *Build hygiene* lines,
   - the agent session rules.

   As a fallback, strip `build-dir` from the export's `.cargo/config.toml`.
2. M63's gate gains a check that an export built that way adds nothing under `$CARGO_HOME/build`, plus a G-base `--self-test` run.
3. Rewrite V4 to measure target-dir and build-dir separately.

### MF-6 — G-base's suite verdict passes a crashed test binary

*Merged from:* F-01.

**What the verdict checks.** `gbase.sh:540` runs the workspace tests through `run_logged_rc 90-test`. The verdict (`:551-571`):

- checks that the failures are exactly P14's seven pins;
- checks for a crash only when `failed` is empty (`:563`);
- prints `LAST_RC` but never asserts it.

The README says to record the pass count, "never hold it to either" (`tools/gbase/README.md:87`).

**The hole.** At the branch point all seven pins fail by design. A test binary that aborts somewhere else prints `error: test failed … Caused by: process didn't exit successfully … (signal: 6, SIGABRT)`, with no FAILED line and no `test result` line of its own. It adds nothing to `unexpected`, so the run passes. The verifier reproduced this with cargo 1.99.0 on a toy workspace.

**Narrowed by the verifier.** A test target that does not type-check is caught earlier: steps 1–8's `cargo check --workspace --all-targets` runs through `run_logged`, which dies on a non-zero exit (`gbase.sh:171-175`, `:476`). Beyond the crash case, only failures at link time remain uncaught.

**Why it matters.** The wave's first gate can report exit 0 while a whole test binary's tests never ran — for example, a crate whose tests stop running after a dependency bump.

**Amendment.**

1. In `final_tests`, require the set of binaries named in `error: test failed, to rerun pass <spec>` to equal the set that holds the expected pins.
2. With an empty pin set, require `LAST_RC == 0`.
3. Die on any `(signal:` line.
4. Hold the passed count to a floor taken from a measured reference, such as a 1.93.0 run of the same export, instead of only recording it.
5. Add a case to `--self-test` with a SIGABRT alongside the pins. Drop the proposed case for a test target that does not compile, or narrow it to a failure at link time.

---

## 4. Should fix (medium)

### Before the cut

**SF-1 — The backup set is rev 2.1's, and it is taken too early** (A1-07, F-02)

*What is wrong.*

- §5.1 item 4 (`plan-rev3.md:629`) copies the database trio, `config/*.toml`, `plugins/.permissions.toml` and `plugins/.config/`. It leaves out:
  - `state/.master_key`, which decrypts llm.toml's `secret_encrypted`;
  - `state/memory-control/`, the notes' authority (a missing or invalid snapshot quarantines its partition);
  - `memory/` and `projects/*/{project.json,memory/}`;
  - `config/orchestrator/*.md`, including USER.md;
  - keychain items, when `use_keychain` is on.
- Those are exactly what P10 (the `hash` module: revision hashes, partition keys), P11 (aes-gcm) and P12 (the keychain store) touch.
- Chunk 25's cut-day item (4) puts the backup before P42, days before the first bumped start, so a restore would lose that interval's rows and notes.
- E8's full copy is deleted at step 9 (`:935`).
- There is no restore procedure: "Backing out" covers git only (`:663`).

*Amendment.* These are owner actions; no agent reads `~/.openalpaca`.

- Immediately before the first start of a bumped build on the real store — daemon stopped, its exit and the lock release awaited — copy the store with `cp -Rp ~/.openalpaca <backup>`, excluding `state/cache/` and `logs/` if wanted. Alternatively, keep E8's copy instead of deleting it.
- Keep the backup at mode 0700, outside any repository, and delete it at the latest when the wave closes.
- Record a checksum list (counts and hashes, never contents) and whether `use_keychain` is on. Note that pointer rows (`in_project = 1`) refer to real project files the backup does not cover.
- Write a restore procedure (stop, move the live store aside, copy back, open with the pre-wave build) and rehearse it once on a scratch copy.
- Move Chunk 25 item (4) after E8.
- Make §5.2's temp-store rule explicit for the owner's own development runs (`cargo run -p openalpacad` and `bun run tauri dev` default to `~/.openalpaca`) until E8 has passed.

**SF-2 — The branch has no CI trigger, and P35's gate can only be read after the merge** (A1-09, E-16, G-01)

*What is wrong.*

- `ci.yml:3-6` runs on push to `main` and on `pull_request` only. The branch was never pushed, and PR #34 is already merged, so a new PR is needed — and no plan or checklist line opens one.
- Several gates therefore have no trigger:
  - P01's "on GitHub all three jobs green … 1.93.0" (`:674`);
  - P42's "bun install v1.4.3" (`:687`);
  - P12's "first Linux compile of zbus" (`:755`);
  - P08's Linux measurement (`:735`);
  - P13's removal of `libdbus-1-dev`.
- Dependabot version updates and `schedule:` workflows read only the default branch. So P35's "the first bun PR must succeed" (`:869`) and DU-13 (a)'s condition (`:522`, `:627`) can be judged only after the merge, while the E-gates are "before the merge" (`:924`).
- DU-33's statement that the Pester job "runs on every push" (`:618`) is wrong.

*Amendment.* These are owner steps; pushing is the owner's, with the owner's consent.

- Before P42/P03, push and open a draft PR against `main`. Record the CI run URLs for P01, P01b and P02.
- Read CI a second time after P12, P13 and P08, before P15. Record each run in Chunk 25.
- Move P35's gate and DU-13 (a)'s condition to an item after the merge, naming the follow-up commit it may trigger.
- Correct DU-33's sentence to "every PR and every push to main".

**SF-3 — The prep commits cannot be rebased into the P commits as written** (E-02, B-04)

*What is wrong.*

- Base `6992a402` is a scratch tree ("not for landing") that already carries every version move: `git diff d7c6bbc6 6992a402` shows 30 files, +1713/−1288. The prep commits are only rev 3.9's deltas over it:
  - `b899046c` ("sha2 0.11") touches no sha2 manifest line.
  - `77d106ec` (P15) and `154b679d` (P18) touch no manifest at all.
  - `5c265232`, `44ec0b03` and `88394660` move only zip 8.6→9.0.2, keyring 4.2 `v1`→keyring-core, and rmcp 3.5.0→3.5.1.
- The only instruction for landing them is `tasks/todo.md:569`: "rebase … in plan order … re-gate each".
- `plan-rev3.md:799` cites P19b `73df38f5` over P19 `88d9de68`, but no ref contains either; the ref holds `e87c038d` and `88394660`. (`c7e03960` is identical to `9ec986ec` by range-diff and drops cleanly.)
- The failure would be loud — conflicts, or failing gates — rather than silent.

*Amendment.*

- Write the landing procedure into §5.1 and Chunk 25. Each P commit is its G-base step's anchored edits and patch hunks, plus the prep delta, rewritten onto the real tip, gated for freshness (MF-2) and re-gated.
- Name which prep commits are pure deltas: P10, P12, P15, P16, P18, P19, P19b and P41.
- Replace the dangling hashes.
- Record that the prep anchors were measured on a newer lock.
- Each entry's own divergences are row A1-05 in §5.

**SF-4 — Chunk 25 is a checklist from before the re-plan** (E-03)

*What is wrong.*

- `tasks/todo.md:542` names plan-rev3 rev 3.5 and modernization rev 1.3.
- The Part III line (`:573`) omits several commits that are Required in modernization §6.3/§6.4 (`modernization-plan.md:951`, `:959-972`): M57-first, M55 (a–d), M58 (a–j), M59, M60–M62, M40, M41, M45–M52, M63–M69, M71–M82 and M64/M65.
- The cut line (`:572`) omits:
  - Q11, which belongs between P31a and P04 (`plan-rev3.md:1086`);
  - M63;
  - DU-M4's `cargo clean` after P03, which appears in neither plan-rev3 nor todo.
- `:574` omits Q11–Q18.
- P28's token, "recorded in Chunk 25 until then" (`:829`), is not there.
- The Part III line ends "(per plan §6)", so an executor who follows the pointer finds the full list. The checklist is still a trap.

*Amendment.* Regenerate Chunk 25 from §7.7 and modernization §6.3 rev 2.0 as one ordered list (see SF-16), with:

- every Required M id, plus Q11–Q18, M63 and the clean;
- the owner items: push points, backups, test tokens, Q00's owed rows, the pdf-extract issue, and Bun 1.3.x;
- the plan revisions it was cut from.

**SF-5 — Q00's person-owed GUI rows are due before P03 and are on no list** (C-05)

*What is wrong.* The baseline README's *What a person still owes the baseline* says those rows must be taken "before P03", because they are "comparable only on this application commit". P03 is the second step of the cut, and neither §5.1 (`plan-rev3.md:624-636`) nor Chunk 25 carries the item. G6's budget and G4's profile depend on these rows (`:1060`, `:1084`). The verifier notes they are not lost: `a91f4b23` can be rebuilt later; the cost is a rebuild.

*Amendment.* Add an owner checkbox before P42/P03. Choose one of:

- take the rows under MF-4's corrected launcher on `a91f4b23`;
- record that they will be taken later on a rebuilt `a91f4b23` bundle;
- record that they are given up, and that Q10 and G4–G6 compare only what Q00 measured.

**SF-6 — A known MCP header leak has no scheduled commit** (A1-03)

*What is wrong.*

- Our MCP transport builds its own reqwest client with the default redirect policy (`crates/openalpaca_mcp/src/transport/http.rs:96-104`) and passes it in through `with_client` (`:112`).
- `HttpAuth::ApiKey` under any header name, and every `extra_headers` entry, ride as default headers (`:57-89`). reqwest strips only Authorization, Cookie and Proxy-Authorization on a cross-origin hop, so those headers are replayed to the redirect target.
- rmcp fixed the same leak in its own default client only (GHSA-9g45-5xwm-f3wc), so adopting rmcp 3.5 does not fix ours.
- The plan records the leak — §3.3 (`plan-rev3.md:449`, "recommended soon as its own fix") and `tasks/pending-decisions.md:251` — but schedules nothing.

*Amendment.* Land its own `fix(mcp):` commit. It moves no version, so it can land before the cut (§6 D6).

- Refuse cross-origin redirects: `Policy::none()` as rmcp does, or a policy that refuses or strips the headers across origins.
- Add a test with a loopback redirect to a second port, asserting the header does not arrive.
- Name the behaviour change in the commit body and in `docs/Daemon_Manual.md`: a server URL that relies on a redirect, including http→https, stops connecting.
- Cite GHSA-9g45-5xwm-f3wc only; GHSA-c9xm-49cp-xcr9 was not verified.

### Before the commit concerned

**SF-7 — The zip advisories are imported: P34's ignore must be live, and E2's (b) clause is wrong** (A2-03, B-05)

*What is wrong.*

- RustSec commit `64f92cd` (2026-10-16T23:27Z) assigned RUSTSEC-2026-0367, -0368 and -0369 (GHSA-vv5m-jxxh-4627, GHSA-4v9h-p7m9-rh26, GHSA-4684-h2q9-2vcp), each patched in zip ≥ 9.0.0.
- calamine 0.36.1, the newest, requires `zip ^8.6`, so zip 8.6.0 stays in the lock and still fires. cargo-deny's workspace scope narrows only unmaintained and unsound advisories, not vulnerabilities.
- P34 keeps its ignore "commented and dormant … the Monday RustSec imports them" (`plan-rev3.md:863`) — and that import has already happened.
- E2's "under (b) zip 9.0.1 is already in" (`:927`) tells the operator nothing is needed under the answer that was taken.
- `:499` ("RustSec has not imported them") is stale. RustSec's record gives GHSA-4684 as patched ≥ 9.0.0, not 9.0.1.
- Reachability: the daemon reads archives only through `by_name`/`by_index` (`refs/prep/deps-wave-2026-10-12:apps/openalpacad/src/extraction.rs:76`, `:143`, `:195`, `:209`). There is no `extract()` or `ZipStreamReader` at HEAD.

*Amendment.*

- Land P34 with the three ids active. Give the reason (no `extract()`/`ZipStreamReader` in the daemon), and lift the ignore when calamine requires `zip ^9`. The ids match only zip < 9, so they cannot hide our own 9.x.
- Give cargo-audit the same three ids, in `.cargo/audit.toml` or as `--ignore` flags.
- Rewrite E2's (b) clause and fix `:499`.
- Close Appendix B #26 with the 2026-10-16 import date.

**SF-8 — Rust 1.99.1 exists, and DU-2 and DU-23 disagree on it** (B-06, F-09, with A1-11, D-16)

*What is wrong.*

- Rust 1.99.1 was published as follows: the stable channel manifest is dated 2026-10-15 (`channel-rust-1.99.1.toml` last-modified 2026-10-15T17:39:23Z), and the GitHub release is dated 2026-10-16T04:47:44Z. It is installed on this machine.
- It fixes:
  - a miscompilation of FFI `bool` return values on x86_64 (#163911) — CI's ubuntu runners are x86_64;
  - the rust-lld libLLVM rpath on macOS (#163947);
  - `wcslen` on UEFI.
- The plan says "no 1.99.1" at `plan-rev3.md:58`, `:103`, `:292` and `:515`.
- DU-2 says to take a 1.99.1 if it appears before the cut (`:517`; `tasks/pending-decisions.md:328`). But DU-23 makes it eligible only from 2026-10-22T17:39Z (by the manifest) or 2026-10-23T04:47Z (by the GitHub release). Which clock applies to a toolchain is not stated.
- Part III's gates hard-code `cargo +1.99.0` (`modernization-plan.md:660`, `:671`, `:679`, `:887`, `:988`).

*Amendment.*

- The owner decides which rule wins (§6 D3).
- Correct the four "no 1.99.1" lines.
- Make the gates say "the pinned toolchain" instead of `+1.99.0`.
- Check whether the tree has an `extern "C"` function returning `bool`, to size CI's exposure to the miscompilation.

**SF-9 — New majors and minors after the evidence: serde-saphyr 2.0.0, jsonschema 0.59, oxlint 1.88** (B-07, E-07, with A2-08, B-10)

*What is wrong.*

- **serde-saphyr 2.0.0** was published 2026-10-13T15:11:53Z (MSRV 1.89) and is eligible from 2026-10-20T15:11:53Z. Its notes list formally breaking changes:
  - a default-on comments feature;
  - a deprecated `AliasError` text field;
  - new `from_multiple*` signatures;
  - granit_parser 2.0.0.

  Yet DU-4 (`plan-rev3.md:478`) and P30 (`:843`) say 1.3.0 is the newest, and E6's yamlcmp pins `=1.3.0` (`tools/e6/yamlcmp/Cargo.toml:17`).
- **jsonschema 0.59.0** (2026-10-16T20:30:14Z) is a breaking 0.x minor, while M82 names 0.58.6 (`modernization-plan.md:869`).
- **oxlint 1.88.0** (2026-10-13T09:47:46Z) has appeared, while M66 names 1.87.0 (`:904`).
- §5.1 item 5 already covers a new major: it is "outside this plan's evidence (pin the researched major, or research first)" (`plan-rev3.md:630`). The rule exists; the record does not.

*Amendment.*

- Record all three in the minute table and in P30, M66 and M82.
- Before P30, re-answer DU-4 (§6 D4). Research 2.0: the option names and `from_str_with_options`, whether to keep comments on or off, the pin test, and yamlcmp rebuilt against 2.0. Then take the newest eligible 2.x. Otherwise name the blocker with its way out.
- Write M66 and M82 as "the newest eligible when the commit lands", with an API and feature check for jsonschema 0.59 (default features off, no remote `$ref`).
- P30's body can also name RUSTSEC-2024-0449 (unsafe-libyaml unmaintained), which P30 clears.

**SF-10 — Four gate commands cannot run as written** (A1-06)

*What is wrong.*

- P12's `cargo tree -i keyring` (`plan-rev3.md:755`) exits 101 with "did not match any packages".
- P16's `cargo tree -p openalpacad -i zip` (`:772`) exits 101 with "ambiguous" (zip 8.6.0 and 9.0.2).
- P13's `-i libdbus-sys` (`:758`) has the same defect: libdbus-sys enters only through tao into the shell.
- P19's `cargo test -p openalpacad managers::mcp routes::tools` (`:793`) fails with "unexpected argument".

*Amendment.* Write absence and duplicate checks as inventories that succeed, as P28 does:

```
cargo tree --locked --workspace --exclude openalpaca_gui --target all -e normal,build --prefix none | grep -c '^keyring v'
```

- That count must be 0. Do the same for `libdbus-sys`.
- Check `-i zip@<9.0.x>` and `-i zip@8.6.0` separately, each with one parent.
- Write P19's command as `cargo test -p openalpacad -- managers::mcp routes::tools`.
- Correct P13's sentence that the check was "not shown to share P28's exit-101 defect".

**SF-11 — P41's entry describes a different commit, and its graph result predates P28** (A2-07)

*What is wrong.*

- Prep `96a1dc30` does not do what P41's entry (`plan-rev3.md:831-836`) says, which is give connectors' optional rustls the `aws_lc_rs` feature. Instead it:
  - adds a `[workspace.dependencies] rustls`;
  - gives openalpacad rustls and a new `apps/openalpacad/src/tls.rs`, installed after the logger;
  - moves connectors' rustls to a dev-dependency (`refs/prep/deps-wave-2026-10-12:crates/openalpaca_connectors/Cargo.toml:20-23`, `:32`).
- The entry's "ring … in use for Telegram's own connections" is wrong. reqwest 0.12 takes the process-wide default provider first (`reqwest-0.12.28/src/async_impl/client.rs:761-771`), so Telegram runs on aws-lc-rs and ring is only compiled.
- The prep sits on a teloxide still using native-tls, so "ring built by nothing" was measured before P28. After P28, reqwest 0.12's `__rustls-ring` enables `rustls/ring` again. That brings back the ambiguous-provider case P41 exists for, which was never exercised.

*Amendment.*

- Update P41 and its §8 row to match `96a1dc30`, and correct the Telegram sentence.
- After P28, re-run `cargo tree -e features -i ring`, the connectors suite, and the daemon tests that build a twilight client.
- Record that the prep's graph result predates P28.

**SF-12 — Required commits can land with their live gate unrun** (E-10)

*What is wrong.*

- P28's gate is a live bot round trip; the token is its named blocker, "recorded in Chunk 25 until then" (`plan-rev3.md:829`). No such item exists in Chunk 25.
- P41's gate needs a live Discord connect (`:833`). Chunk 25 reduces that to "if a token exists" (`tasks/todo.md:569`), and the prep body says the gate was not run (no bot token).
- P19b's real-server check is "if one exists by then" (`:802`).
- The cut list places P28 and P41 inline.

*Amendment.*

- Add open owner items for a Telegram and a Discord test-bot token. P28 and P41 wait for them, or carry a recorded waiver (§6 D10).
- Make P19b's in-process stub its gate of record, and its real-server check an E-step.
- Add a Discord row to the "what CI cannot cover" table (A2-06).

**SF-13 — Live checks keep the owner's credentials out by a configuration nobody specifies or checks** (F-05, with F-06)

*What is wrong.*

- §5.2 requires "credential discovery off in the seeded llm.toml" (`plan-rev3.md:643-644`) but gives no mechanism and no check.
- The seed template has no `[credential_discovery]` and no `[cli_backends]` section.
- `build_token_manager` runs discovery unless both sources are explicitly false (`apps/openalpacad/src/services/llm.rs:178`). It runs at boot and every 30 s (`crates/openalpaca_llm/src/keys/credential_discovery/mod.rs:119`), and on macOS falls back to `security find-generic-password` (`:511`).
- With no `[cli_backends]`, the router registers whatever `which claude` / `which codex` finds (`router_builder.rs:327-360`). A failed Anthropic or OpenAI call then falls back to running those CLIs with the daemon's HOME (`fallback.rs:30-65`).
- `loopback-only.sb` denies only outbound network.
- G-bundle leg B lets the sidecar seed llm.toml itself, and E5 adds a key to a keyed provider.
- Q00 did pre-seed both off and check the log; the plan just does not say so. Network denial keeps the tokens from leaving the machine, but discovery can still copy them into the test store and its logs.

*Amendment.*

- Write the exact pre-seed in §5.2: `[credential_discovery] claude_code = false, codex = false`, plus `enabled = false` under both `[cli_backends.*]`. It must be in place before the first launch, for every launcher including leg B, `tauri dev` and E5.
- Make it a pass condition that `daemon.log` has no "TokenManager initialized" and no "Auto-detected … CLI" / "Registered … CLI backend" line.
- Give live daemons a scratch HOME.
- A stricter profile that also denies reads under `~/.claude`, `~/.codex` and `~/.openalpaca` would be stronger. Check that it does not break the app/webview launcher before relying on it.
- F-06: every test router also registers the real CLIs, though no test reaches the fallback today (the llm_config tests are Ollama-only). Give CLI-backend detection a host, as was done for credential discovery (DiscoveryHost), and record it in §5.2's residue until then.

**SF-14 — The DU-10 keychain count covers llm.toml only** (F-07)

*What is wrong.*

- The count loop (`tools/e6/README.md:136-146`) greps `^\s*secret_ref\s*=` over llm.toml.
- The plugin manager is given the daemon's secret store (`apps/openalpacad/src/main.rs:433`). It writes keychain items named `openalpaca-plugin-<name>-<key>` and records each reference as an inline table, `key = { secret_ref = … }`, in `plugins/.config/<name>.toml` (`crates/openalpaca_plugins/src/manager.rs:2286-2300`; `permission_gate.rs:446-454`).
- Those references are outside the counted files, and the inline-table spelling also fails the line-anchored regex.
- A count of 0 waives P12's only round trip against the real keychain (`plan-rev3.md:755`).

*Amendment.* Count `secret_ref` keys at any depth, with a TOML parser, over llm.toml, `plugins/.config/*.toml` and `plugins/.permissions.toml`, reporting counts only. Until then, P12's waiver needs a 0 on both the llm.toml count and a plugin-config count.

**SF-15 — E8 step 7 cannot run, and its probe is not scheduled** (F-03, with A2-17, F-04)

*What is wrong.*

- FTS5's integrity-check is an INSERT. On a `mode=ro` connection it fails with "attempt to write a readonly database" (measured).
- `PRAGMA integrity_check` already runs FTS5's integrity check read-only, since SQLite 3.44.
- No probe exists under `tasks/deps-upgrade/tools/` (only `e6/` and `gbase/`), and Chunk 25 has only "E1–E8 and the review section here" (`tasks/todo.md:575`).
- A2-17: step 3's `sqlite3 'file:$T/…?mode=ro'` single-quotes `$T`, so the shell never expands it (`plan-rev3.md:935`).
- F-04: E8 has no output rule. The doctor prints partition roots and note file names, and step 3 inventories project roots by value. The evidence tarballs are git-ignored (`.gitignore:139`), so this would reach the transcript, not git history.

*Amendment.*

- Schedule the probe as a deliverable that moves no version, reviewed like the E6 tools: old and new variants built from the branch point, differing only in rusqlite and sha2.
- Run `PRAGMA integrity_check` and `foreign_key_check` read-only. Run FTS5's INSERT form only on a second scratch copy opened read-write.
- Use double quotes in step 3, and do not add `immutable=1`: it skips the copied WAL and would undercount.
- Give E8 E6's output rule: counts, health words and partition keys only, with the doctor's full report saved outside the repository.

**SF-16 — There is no single ordered list for the work after P38** (E-08, with D-08, D-18, E-14, C-10)

*What is wrong.*

- The re-plan lists Part II and Part III as independent sequences (`reports/2026-10-12-no-compromise-replan.md:231-233`).
- The interlocks are given only pairwise (`modernization-plan.md:952`):
  - M18 before Q05;
  - M19/M20 after Q06;
  - Q11 before M74;
  - Q13 before M76;
  - Q18 before M68.
- M64 is "after every other source commit" (`:893`), but its reason names only Part I/III anchors, while Part II's Q04 and Q12–Q14 cite `lib.rs` line ranges.
- M65's warnings gate (`:898`) binds every later Q commit.
- E5's Export click comes before M76 (`:848`), which splits E5 around a Part III commit.
- Q11 is placed in three different ways across the documents.

*Amendment.* Publish one merged total order after P38, in §7.7, in modernization §6.3 and in Chunk 25:

- Q12–Q14, Q04, Q05 and Q06 interleaved with M18, M19 and M20.
- M64 and M65 after the last Q source commit. Alternatively, run M64 before M45–M53 with each crate's site list re-derived by a fresh clippy run, which saves the hand reflows (D-08).
- E5 split around M76, and E1–E8 at the very end.
- Q11 in exactly one place.
- M77 after M79, or M77 adding the shell's `opener` dependency itself (D-18).
- M08's heading changed to "after P38" (E-14).
- Add Q04–Q07 and M72–M79 to the G-GUI row (C-10).

### Before the end-to-end gates, and records to correct now

**SF-17 — The end-to-end gates carry anchors that have since been withdrawn** (A2-05, A2-06, B-11, E-04, F-11, with D-14)

*What is wrong.*

- E3 (`plan-rev3.md:928`) still says "66 unique (≤ 43 after P36)" and "1585 fmt hunks (equal to the base)", and its matrix lists "rmcp 3.5.0 (3.5.1 left to the bot), Bun 1.4.2".
- E5 step 3 (`:930`) requires the marker `rmcp-3.5.0` ≥ 1.
- E1 (`:926`) requires a Windows check that DU-33 defers (`:677`, `:1008`).
- The "what CI cannot cover" table's Windows rows and its "P28's gate, if taken" row (`:914`, `:919`) predate rev 3.9.
- §5.1 item 6 (`:631`) still describes P04's ten-crate hold, `--precise 3.5.0` twice, and a comparison with `recut-lock-99-final-held.lock`, a file that has been deleted.
- The current anchors are different:
  - G-lint: 78 → 101 → 100, and "no commit adds a hunk" (`:658`);
  - G-ident: lock-derived markers, `rmcp-3.5.1` (`:659`);
  - P36: 100 → 0 (`:875`);
  - P42: Bun 1.4.3 (`:685`).
- Most of the stale anchors fail closed (clippy 0 ≠ 66). E1 cannot be met at all. zip 8.6 (under calamine) and lopdf 0.42 are still valid holds.

*Amendment.*

- **E1:** under DU-33 — CI's existing jobs, Pester, and the macOS shell-check leg.
- **E3:** clippy 0 after P36; no fmt hunk added, and 0 hunks after M64; the matrix from §3.1 at rev 3.12 (rmcp 3.5.x, Bun 1.4.3, calamine's zip 8.6, lopdf 0.42 against 0.45).
- **E5:** markers derived from the lock.
- Mark §5.1 item 6 superseded by 6b.
- Mark P27a's Linux Open as deferred.
- D-14: update modernization's stale rows — §3.1 row 4, M07's "5,080 / 0 / 16", §6.3's "Recommended" — and quote gate counts as "the parent's count, 0 failed".

**SF-18 — The decision ledger and the read-first record state superseded answers** (E-05, E-11, with E-19)

*What is wrong.*

- `tasks/api-fix-plan.md` §0's eleventh subsection (rev 3.26, `:250`) records as taken several answers that the 2026-10-12 re-plan reversed:
  - the ten-crate Tauri hold, rmcp 3.5.0 with the pin, and zip held at 8.6 (`:257`);
  - Bun 1.4.2 (`:256`);
  - no dynosaur, rmcp's opt-ins deferred, no React Compiler, and no Trusted Types (`:262`).
- Nothing points to their supersession. DU-33 is absent there and in modernization-plan.md.
- P38 (`plan-rev3.md:881`) and §4 (`:467`) still say to "add" that subsection, although it exists.
- CLAUDE.md's *Current Work* ends at rev 3.10. It does not mention:
  - `9ec986ec`;
  - the rule that a test reaches credential discovery only through `DiscoveryHost` (§5.2, `:643`);
  - the daemon-test residue;
  - `refs/prep` and its never-push rule;
  - P19b.
- `74b86219` and `e4cfd37c` are named only in the git-ignored `tasks/todo.md`.

*Amendment.* Make one docs commit now; it moves no version.

- Add a 2026-10-12 row to api-fix-plan §0 listing the re-answers and DU-33, with `d7f21b65` and `a91f4b23`. Mark `:257` and `:262` superseded. Change P38, §4 and modernization §5 to "update" the subsection.
- Bring CLAUDE.md's *Current Work* to rev 3.12 / 2.1:
  - the keychain fix;
  - the prep ref and its rule;
  - P19b;
  - M08 after P38;
  - P36 100 → 0;
  - a *Gotchas* bullet on credential discovery and `DiscoveryHost`.
- Name `74b86219` and `e4cfd37c` in plan-rev3's revision rows.
- P19b's own status is §6 D5.

**SF-19 — plan-rev3's body still states superseded holds as current** (E-09, with C-07, D-11)

*What is wrong.* Lines that read as current instructions but have been superseded:

| Where | What it still says |
|---|---|
| `:118` | keyring `features = ["v1"]` |
| `:121` | zip "hold 8.6.0" |
| `:123` | rmcp without a `get_info` override |
| `:138-140` | tauri "hold 2.11.6" |
| `:173`, `:357` | "P33 stays optional", against `:857`, which withdraws it |
| `:530` | DU-23 keeps rmcp 3.5.0 and Bun 1.4.2 below the latest "by choice" |
| `:957`, `:962` | §7.2 "storage re-exports every moved item" and "`memory_files::fsx` and `sha256_hex` re-exported", against `:1002`'s no re-exports |
| `tools/gbase/README.md` *Scope* | "P19's protocol-offer pin" and "P23/P28 (optional)" |

Many other stale lines are dated measurement records, and a reader can tell them apart.

*Amendment.*

- Mark the lines above superseded inline, as §3.1 (`:427`) and §3.2 (`:442`) already do for their rows.
- Add one sentence at the head of §1 and §2 saying they are dated records, and that §5.3 and the revision log from rev 3.9 on are authoritative.
- Do the same for DU-27's facts bullet (`:583`, C-07), and for M39's first Contents bullet and §3.5 items 7 and 9 (`modernization-plan.md:762`, `:458`, `:460`, D-11).

**SF-20 — P10's `hash` module is not where Q06 expects the helper** (C-08, E-06)

*What is wrong.*

- Prep `b899046c` creates `openalpaca_storage::hash::{sha256_hex, lower_hex}` (`refs/prep/deps-wave-2026-10-12:crates/openalpaca_storage/src/hash.rs:16`) and re-points every caller to it, with no re-export.
- One of those callers is `store/projects.rs:170-171` (`path_hash`). That is the reader Q06 moves into `openalpaca_app_support`, which may not depend on storage (`plan-rev3.md:957`).
- Q06's table (`:962`) names `support::fsx … sha256_hex` and says nothing of `hash` or `lower_hex`. Rev 3.9 forbids re-exports (`:1002`).
- Callers span core (`memory/service`, `session_log/writer.rs`, `file_ops.rs`) and the daemon (`hot_reload.rs`, `managers/mcp.rs`).

*Amendment.*

- Keep P10 as written; the support crate does not exist yet.
- Rewrite Q06 and §7.2's table to move `hash` whole, with its known-answer test, to `openalpaca_app_support::hash`, with storage importing it from support.
- List P10's re-pointed sites in Q06.
- State that core and the daemon gain a dependency on support. That widens those components, which Q06 must name; the only alternative is a forbidden re-export.
- Keep `rg -c 'fn sha256_hex' crates apps` → 1 in Q06's gate, and fix `:957`.

**SF-21 — DevServerHost hands the daemon token to pages on other localhost ports** (C-02)

*What is wrong.*

- Vite's default `server.cors` is `{ origin: defaultAllowedOrigins }` (`apps/openalpaca-gui/node_modules/vite/dist/node/chunks/config.js:25780`, Vite 7.3.6). That regex admits localhost, `*.localhost`, 127.0.0.1 and `[::1]` on any port (`logger.js:224`), and the CORS middleware is installed before the `configureServer` hooks run (`config.js:25673-25682`).
- G3-a's rule "answer only 127.0.0.1/localhost origins" (`plan-rev3.md:970`) pins no port, and the daemon's CORS is `permissive()` (`apps/openalpacad/src/router.rs:486-488`).
- So a page on any other local dev server can read `/__openalpaca/connection` cross-origin. That contradicts DU-28's sentence that the Origin check binds browsers (`:589`).
- The case where `OPENALPACA_HOME_STORE` is unset is not specified. A fallback to `home_root()` would serve the owner's real token.
- Limits on the risk: the widening DU-28 (a) disclosed (`tauri dev`, local curl) still stands; `allowedHosts` blocks DNS rebinding; and nothing is built yet (Q07).

*Amendment.*

- Register the plugin only when `VITE_OPENALPACA_HOST=dev-server`.
- In that mode set `server.cors: false`, and require `Origin` to be exactly `http://localhost:1420` or `http://127.0.0.1:1420`.
- Refuse unless `OPENALPACA_HOME_STORE` is set and absolute, with no fallback. Apply the same rules to the log-tail endpoint.
- Correct DU-28's sentence, and add a cross-origin fetch from another localhost port to Q07's gate.

**SF-22 — Part II's startup scenario and its fixture have no runnable protocol** (C-04, C-06)

*What is wrong.*

- Scenario (1), cold launch (`plan-rev3.md:1058`), needs a webview that connects and no daemon already running.
  - Under launcher (iii) the webview never connects: Q00 found that WebKit's Networking process sets its own block-network flag, loopback connects fail with EPERM, and 8 of 8 launches had `connected_clients` 0.
  - Under rev 3.8's launcher a daemon is already running, which is scenario (2).
  - §7.5 names no launcher per scenario, and G6's time-to-interactive (`:1050`) inherits the gap.
- G7's fixture is to be "seeded through the HTTP API" (`:1054`), with 100 messages of long text, code and tables.
  - No route writes messages without a model turn (`router.rs:197-235`), and the deterministic commands give only short fixed rows.
  - Q00 built no such fixture. G4 (`:1048`) and G5's 500-message fixture (`:1049`) rest on it.

*Amendment.*

- Give each §7.5 scenario its launcher. Scenario (1) uses MF-4's confined-sidecar wrapper, with the wrapper's cost reported separately. Launcher (iii) is kept for daemon-ready only. First paint also needs an instrumented build or a person.
- Specify a deterministic seeder. Either:
  - a canned OpenAI-compatible stub on loopback as the only enabled provider, which also makes scenario (4)'s 2,000-token stream reproducible; or
  - a test-only storage seeder run before the daemon starts.
- Build the fixture once, archive it, and copy it for each trial.

**SF-23 — Q07 rewires every Tauri call with no check in a real webview before E5** (C-11)

*What is wrong.*

- G-bundle's attachment list (`plan-rev3.md:661`) omits Q07.
- Q07's gate is the bun gates, a TauriHost suite on `mockIPC` (whose ACL authority is empty, `:1016`), and "the Tauri path unchanged in E5" (`:1005`).
- E5 comes after Q08–Q18 (`:1086`), so a broken TauriHost would be found only after all of those were built on top of it.

*Amendment.*

- Attach G-bundle leg A to Q07: connect, the log tail, stop and start under MF-4's launcher, Reveal, the folder picker, no duplicate window chrome, and — once M74 has landed — a refused ungranted invoke.
- For Q06, the moved, byte-pinned refusal sentences are the required gate. A leg A Reveal check there is optional.

### Part III

**SF-24 — M57 as written does not compile, and it multiplies the request timeout** (D-03, D-04)

*What is wrong.*

- `LlmError` and `LlmRouterError` derive `Clone` (`crates/openalpaca_llm/src/error.rs:1`; `routing/router/types.rs:68-69`, `:103-108`), and `reqwest::Error` is not `Clone`. So "typed LlmError variants that keep the error as #[source]" (`modernization-plan.md:798`) breaks the derive.
- Today a reqwest timeout's Display matches no substring in the classifier, so the call fails once. Once real timeouts are classified as transient:
  - the router retries `max(pool.len(), 3)` times with backoff (`retry.rs:33`, `:203-218`);
  - `complete()` then walks the fallback chain on `MaxRetriesExceeded` (`completion.rs:172-178`);
  - the agentic loop retries 3 more times (`agentic_loop/mod.rs:453`, `:1429-1440`).
- Limits on that: background callers such as `working_state.rs:510` call the router directly, so they get about 3× plus the fallbacks, not 12×; the circuit breaker fails fast once open; and retrying timeouts restores what the classifier's comment says was intended.

*Amendment.*

- Name the source type in M57: `Arc<reqwest::Error>`, or a typed kind plus a message with no source. Display must not repeat the source's text.
- Write M57's retry table into the plan: which kinds retry at which layer, and whether a timeout that used the full `request_timeout` is retried on the same key.
- Bound the total wall time, and test it against a silent listener, counting the fallback chain and the circuit breaker.
- Record the latency change in the commit body, under L7.

**SF-25 — M19–M38's `#[source]` rule doubles error text through wrappers** (D-05)

*What is wrong.*

- M19–M38 adds `#[source]` "where a cause is already carried and swallowed" (`modernization-plan.md:758`).
- `IndexError::Sqlite` and `IndexError::Io` already print their cause, and today their `source()` is `None` (`memory_index/mod.rs:127-143`).
- `MemoryError::Internal` wraps an `IndexError` through `anyhow::Error::new` and prints it with `{e:#}` (`service/error.rs:147`, `:184-191`), which walks `source()`. With a source added, the text becomes "memory index database: X: X".
- The per-type golden test stays green, and a later `MemoryError` golden test would lock in the doubled text.
- §3.1 row 13 (`:379`) says the source becomes expressible "later", which conflicts with M19–M38 adding it now.

*Amendment.*

- Choose one rule and state it. Either M19–M38 add no `#[source]` (byte-identical text), or a variant that gains `#[source]` drops the cause from its `#[error]` text in the same commit.
- Either way, write golden tests of the `{:#}` text through every wrapper before M21–M23.
- §3.5 item 1's rule for memory code applies: read the memory design before touching it.

**SF-26 — M61 breaks a test it does not name** (D-02)

*What is wrong.* clap_derive 4.6.7 defers only the subcommand's argument augmentation (`derives/subcommand.rs:335-339`) and applies the about text outside the deferral (`:349-356`).

- So `tasks_resume_help_names_the_experimental_replay` (`apps/openalpaca/src/commands/tasks.rs:885-894`, which calls `get_about()`) keeps passing.
- But `tasks_list_status_help_names_interrupted` (`:822-841`, which calls `get_arguments()` on an unbuilt deferred subcommand) panics.
- Neither test calls `.build()` today, contrary to M61's text (`modernization-plan.md:700`).
- M61's own gate catches the failure.

*Amendment.* Build the command in that test, correct M61's sentence, and audit every test that reads arguments from an unbuilt derived command.

**SF-27 — M42's skipToken recipe drops the non-id halves of six compound guards** (D-07)

*What is wrong.* The recipe says `id === null ? skipToken : …`, with "the `enabled:` line … removed" (`modernization-plan.md:774`). But six hooks have compound guards:

| Hook | What the extra condition does |
|---|---|
| useArtifactDiff (`hooks/useArtifacts.ts:176`) | the range check |
| useArtifactText (`:189`, `:194`) | the caller's large-file/binary preview policy |
| useMemoryRevision (`useMemories.ts:208`) | `revision !== null` |
| `useTasks.ts:64` | `!== ""` |
| `useEventHistory.ts:41` | `!== ""` |
| `useFollowups.ts:35` | `!== ""` |

Applied literally, the recipe fetches binaries and very large files into the webview, and requests invalid diffs.

*Amendment.* The `skipToken` condition is the whole former `enabled` predicate. List the six sites, and test for each that nothing is fetched while any part of the predicate is false.

**SF-28 — M65's warnings gate will hit a known Linux-only warning** (G-02)

*What is wrong.*

- `apps/openalpacad/src/notification/imessage.rs:3-5` imports unconditionally, while every use is under `cfg(target_os = "macos")` (`:12`, `:61`, `:65`, `:130`). On Linux those imports are `unused_imports`.
- §3.3 records this as separate work (`plan-rev3.md:449`), and no commit fixes it.
- M65 sets `CARGO_BUILD_WARNINGS: deny` on CI's rust job (ubuntu-24.04), with the gate "0 warnings under every selection M39 names" (`modernization-plan.md:897-900`). Every one of those selections is a macOS build (`:761-763`).
- 254 `cfg(target_os/unix/windows)` sites in 54 files have never been measured on Linux. An `#[expect]` that is fulfilled only on macOS becomes `unfulfilled_lint_expectations` on Linux.

*Amendment.*

- Gate imessage.rs's imports with `cfg` before M65.
- Add CI's Linux job, or a local `--target x86_64-unknown-linux-gnu` check, to M39's and M65's selections.
- Read CI's clippy log on the draft PR (SF-2) for Linux-only warnings before M39 converts any `allow`.

**SF-29 — DU-13 (b)'s fallback has no Bun to run on** (A2-09, with A1-08, E-12)

*Severity.* Medium as merged: A2-09 was verified medium; A1-08 and E-12 were verified low.

*What is wrong.*

- The machine holds only `~/.bun`, at 1.4.3. `~/.bun-1.4.2` does not exist, and no 1.3.x is installed.
- §5.1 item 2 (`plan-rev3.md:627`), DU-13 (b)'s prerequisite (`:522`) and P35's gate (`:869`) all count on a 1.3.13 install.
- Chunk 25's checked item "Bun 1.4.2 beside 1.3.13 at `~/.bun-1.4.2/bin/bun`" (`tasks/todo.md:549`) describes a state that no longer exists.
- dependabot-core bundles Bun 1.4.2, while P42 pins 1.4.3. Whether Dependabot accepts the newer pin is unknown.
- The fallback matters only if the first hosted bun run fails, and that run happens only after the merge (SF-2).

*Amendment.*

- State that (b) now needs a fresh Bun 1.3.x in its own `BUN_INSTALL` root. The owner installs it before P35 if the fallback is to stay real; otherwise record that (b) needs that install at the time it triggers (§6 D12).
- Drop the 1.4.2 side install after P42.
- Add "Dependabot's bundled 1.4.2 against `packageManager` 1.4.3" as a second failure mode in P35's gate.
- Correct Chunk 25.

---

## 5. Minor (low)

| Id(s) | Location | Defect → fix |
|---|---|---|
| A1-04 | `plan-rev3.md:787`, `:791` | P19's process-group text (`ProcessGroup::leader()` alone) fails P19's own pass condition (`:795`). rmcp closes stdin, waits 3 s and kills only a server still running, so prep `88394660` stacks `group::EndGroupWithLeader`. → Rewrite `:787` to the prep's design: leader plus EndGroupWithLeader, a JobObject on Windows, and the post-reap SIGKILL pgid window recorded as residue. Add the `connect_timeout_secs ≤ 10` consequence to `:791`; the docs already carry it. |
| A1-05 | `plan-rev3.md:743-745`, `:754-756`, `:771-772`, `:799-803` | Entries lack their prep bodies' "disagree" lists. **P10:** `writer.rs:654` is a streaming hash; `file_ops.rs:637` was rewritten; the 0.10 line is held by lopdf and tauri-codegen/wry; openalpacad's sha2 line goes; P05's known-answer test must be renamed. **P12:** the zbus store needs `crypto-rust`; its cfg is every unix but macOS/iOS/Android; the construction sites are `apps/openalpacad/src/services/llm.rs:25`, `:68`, not `services/mod.rs:122`; the store is a `NATIVE_STORE` LazyLock. **P16:** zstd-sys and zstd-safe stay (twilight-gateway). **P19b:** the hashes (SF-3). → Fold each body's list into its entry; bump to rev 3.13. |
| A1-10 | `plan-rev3.md:744`, `:657`, `:740` | sha2 0.11 and base64 0.23 reach the shell through openalpaca_storage (`crates/openalpaca_storage/Cargo.toml:16`, `:21`; shell manifest `:24`). By the G-GUI row's own rule, P09 and P10 trigger it. → Run G-GUI after P10 or at the end of Phase E; fix the rationale. |
| A1-12 | `plan-rev3.md:803` | P19b's residue: a list change announced between `serve` and the `listen` acknowledgment is not heard until some unrelated change. That reopens, for one window, the X-35 hole P19b exists to close (`:800`). → On acknowledgment, send one ServerChange (one `tools/list`) through the existing channel. Add a stub test that announces a change before acknowledging. |
| A1-14, A2-13 | `README.md:29`, `:211`; `plan-rev3.md:686`, `:881-883`; `CLAUDE.md:56` | P42 leaves README's two "Bun 1.4.2" lines. P38's `git grep -n '1\.93'` gate matches CLAUDE.md:56's programme paragraph (on no commit's list) and the binary `rust_out` (removed only by the Recommended P37). → Add README `:29`/`:211` to P42 and §8's P42 row. P38 rewrites CLAUDE.md:56. Exclude `rust_out` in the gate, or make P37 a prerequisite. |
| A1-15, A2-14, C-22 | plan-rev3 §5.1–§5.3, §8 (`:1098-1175`), Part II | Citations taken at `7295f91b` have drifted. CLAUDE.md's toolchain and CI lines are `:178`/`:180`, not `:168`/`:170`. `Cargo.toml` rust-version is `:37`, not `:32`, and the dependency lines are +7. The shell's tauri-build/tauri are `:12`/`:23`, not `:16`/`:27`. CorsLayer::permissive is `router.rs:487`. P14 is still "the one unwritten Required commit", though it landed as `05541f1b`. §7.9 is cited, but §7 ends at §7.8. §7.3 lists Q01–Q03 as landed, not Q15–Q17. Q05's "3 discovery" tests are 5. `--reuse <path>` was built as a flag without a path. P37's release-script item was already done by Q03. CLAUDE.md still cites plan-rev3 rev 3.5 and modernization rev 1.3. (`router.rs:480`, `.layer(cors_layer())`, is still right.) → Re-cite at the cut's tip; mark landed rows done; drop P37's item; fix Q05's count. |
| A1-16 | `plan-rev3.md:768`; prep `77d106ec` | P15's `#[ignore = "… upstream <issue>"]` points at an issue nobody has filed, and no item assigns the filing. → Owner action in Chunk 25 and pending-decisions: file the pdf-extract issue with the two synthetic fixtures (fixtures only; the owner posts). Put the URL in the ignore reason before P15, or record why not. |
| A1-17 | `plan-rev3.md:632`, `:634`; `tools/gbase/README.md:40` | The run of record is recorded only in the git-ignored todo.md, and its logs sit in a mktemp root. → Add to 6a and Chunk 25: archive `$T/logs` and the lock-difference and dates files as `evidence-<date>-gbase.tar.gz`, indexed in `tasks/deps-upgrade/README.md`. |
| A2-10 | `plan-rev3.md:873-876` | P36's contents still list the withdrawn "+23 sites" scope (`:874`), while the gate is 100 → 0 (`:875`). P36 alone never ran the workspace suite; it ran once, on the tip with M08. → Contents as M04 + M07. Re-measure clippy and fmt, and run G-CI on P36 alone, at its real Phase L parent. Explain 1585/1586 by searching `7295f91b..3598a6fb` plus `6992a402`'s re-cut drift. |
| A2-11 | `plan-rev3.md:930`; `prepare-sidecar.ts:56-70`; `package-macos.sh:146-188` | With `CARGO_TARGET_DIR=target/gui` exported for `tauri build`, the `beforeBuildCommand` hook rebuilds and re-hashes the daemon, so G-ident can end up comparing a build with itself. The "Without them" branch is dead since Q02/Q03. → Replicate package-macos.sh's sequence by hand (prepare-sidecar --release, record the hash, `bunx tauri build --bundles app --config '{"identifier":"…"}'` with the variable unexported), or add a `--config` passthrough to the script. Delete the dead branch. |
| A2-12 | `plan-rev3.md:847-849` | P31's anchors were taken before bunfig.toml's minimumReleaseAge and before many JS releases: bun audit 0, the CSS file name, 19 chunks / 773,536 B, and "`bun outdated` lists vite and vitest only". → Anchor "every `bun outdated` row is explained by DU-23's table at the cut". Record P31's own figures under bunfig. Check react's coordinated 2026-10-14 releases (19.3.1, 19.2.9, 19.1.10, 19.0.9) for an advisory. |
| A2-15 | `plan-rev3.md:821-824`; `apps/openalpacad/src/routes/files_types.rs:146-157`, `:183` | P27a: "our route stages the file first, so no change for us". Only uploads are staged (`stage = origin != Produced`); by-products and result files open in place. opener's reaper thread is not used on macOS. → Correct P27a and DU-24's sentence. Keep the Linux Open check, deferred under DU-33. |
| A2-16 | `plan-rev3.md:924`, `:933`; `tools/e6/README.md:44`, `:62-64` | E6 sits in the after-P38 list though it runs at the consent point. typedprobe builds with `cargo +1.93.0`, which fails on a post-P03 tree. → Move E6 to §5.1 item 3. Have the README take the toolchain from `rust-toolchain.toml`. |
| B-08 | `plan-rev3.md:160`, `:849`, `:865`, `:927` | The "bun audit 0" anchors rest on advisories npm has not imported. **dompurify ≤ 3.4.16:** three low advisories, fixed in 3.4.17, eligible from 10-19T12:41:28Z. **@vitest/mocker < 4.1.12:** two medium; P31's interim keeps vitest 4.1.11. **vite 7.3.6:** not verified (rate-limited). Our DOMPurify calls use default options. → Name them, with reachability, in P31 and P34. Take dompurify 3.4.17 when eligible. Check vite 7.3.6 with an authenticated request; 7.3.7 has been eligible since 10-13. Add ignores only for what cannot be moved. |
| C-03 | `plan-rev3.md:644`, `:1013`, `:1069`, `:1266`; `scripts/eval/memory/loopback-only.sb:1-3` | §5.2's launcher (iii), the profile's header comment and Appendix B #41 still read as they did before Q00's finding that WebKit's Networking process blocks loopback. Q09's checks remain valid under (iii). → Correct all three. Keep (iii) for spawn and daemon-ready. An assertion that the webview connected needs MF-4's wrapper leg. Record whether `sandbox-exec` works on macos-latest. |
| C-09 | `plan-rev3.md:591-592`, `:972`; `src-tauri/src/lib.rs:254-268`, `:326` | `decide_reuse` spawns over a daemon that is alive but slow (health over 300 ms). The log is rotated under an open fd, the duplicate is refused by the lock, and the poll loop then recovers. → Add a row: the discovery pid is a live `openalpacad` (`running_daemon_pid()`, which has no side effect) but health does not answer → "unresponsive": park, retry, never spawn, no rotation. Do not use `singleton_lock_is_free()` as a first step; it takes the lock and can make a booting daemon exit. An unparseable discovery file over a live pid is a mismatch candidate. |
| C-12 | `plan-rev3.md:1016`, `:1086`; `src-tauri/src/lib.rs:287-299` | Q11's `ensure_daemon_running` round trip under `tauri::test` can reach the debug PATH fallback and spawn a real setsid daemon. This needs an `openalpacad` on PATH; the installer puts it in libexec. → Test only the reuse and refusal paths, or make the spawner and locator injectable. Name the rule in Q11's body. |
| C-13 | `plan-rev3.md:953`, `:962`, `:1093`; `src/lib/reveal.ts:4-10` | Moving Reveal into the daemon, as a route by file id, is never weighed. → §6 D8. |
| C-14 | `plan-rev3.md:996`; `tauri-utils-2.10.1/src/platform/starting_binary.rs:20-37`, `:66-80` | Q04's use of Tauri's `current_exe()` refuses on macOS when any ancestor directory is a symlink. That is a new refusal for the sidecar locator. → §6 D7. Add a case with a symlinked ancestor to leg B. |
| C-15 | `plan-rev3.md:996`, `:1019`; `Cargo.toml:52-60` | The shell's tokio has no `net` feature, so Q12's `tokio::net::TcpStream` does not compile. → `tokio = { workspace = true, features = ["net"] }` in the shell manifest, with the `cargo tree` delta in the commit body. Enabling a feature is not a new dependency. |
| C-16 | `plan-rev3.md:1048`, `:1050`, `:1060` | React Profiler traces and Web Inspector figures cannot be taken on release builds without react-dom's profiling build or Tauri's `devtools` feature. → Name the instrumented build in §7.4/§7.5, used identically at baseline and after. Record its provenance in Q10. |
| C-17 | `plan-rev3.md:1008-1009`, `:1012-1013`, `:1068-1069` | DU-33's deferral appears only in Q08's header. Q08's gate, Q09 and §7.6 are still written for three operating systems. → Reword as macOS-only, mark the Windows and Linux rows deferred, adjust the cost line. |
| C-18 | `tasks/pending-decisions.md:310`; `plan-rev3.md:608`, `:1039-1040`; `scripts/release/package-macos.sh:103-116`; `prepare-sidecar.ts:39-40` | Owner question (c)-2 omits that each package is built for its host architecture only. There is no universal build, so a release built on Apple silicon excludes Intel Macs. Q18's Info.plist half does not depend on P32: Tailwind 4.3.3 already requires Safari 16.4. → Add the fact to (c)-2 and Q18. Split Q18: `minimumSystemVersion` once the number is given; the Vite/tsconfig half after P32. |
| C-19, E-13 | `tasks/pending-decisions.md:312-313`; `modernization-plan.md:709-710`, `:838-858` | Owner question (c)-5 appears in neither plan. M74, M76 and M78 are written under its "no change" answer (a) without saying so. Option (b) needs window permissions M74 does not list. Browser hosts render inert window controls until Q07. M68's ES-year sentence is undefined while Q18 is unanswered. → Record (c)-5 in modernization §5. State that M74/M76/M78 are written under (c)-5 (a) and that (b) adds a follow-up. For M68, keep today's target/lib while Q18 is open, or order M68 after Q18. |
| C-20 | `plan-rev3.md:972`, `:995`, `:1086` | Q04's "test-only env override" for PROTOCOL + 1 has no stated exclusion from release builds. `support::protocol` is created in storage and then moved by Q05. → Gate the override with `#[cfg(debug_assertions)]` or a test feature. Land Q05 first, or say that Q05 re-points the callers. |
| C-21 | `plan-rev3.md:957`, `:999`; `crates/openalpaca_storage/src/discovery/mod.rs:11-12`, `:75-78` | Under W1, `generate_token` moves to support, yet the support crate's manifest list omits base64 and rand. Q05's sentence about the shell's manifest is ambiguous. → Add base64 and rand. Q05 removes the unused `directories` and `serde_json`; the storage → support switch happens once, in Q06. |
| C-23 | CLAUDE.md *Dependency Graph*; `plan-rev3.md:999`, `:1002`; shell `Cargo.toml:14-28` | The shell does not depend on openalpaca_api, but CLAUDE.md and Q05/Q06's planned edits say it does. → `openalpaca_gui ← openalpaca_storage` now, `← openalpaca_app_support` after Q06. |
| C-24 | `src-tauri/tauri.conf.json:24`; `apps/openalpacad/src/router.rs:486-488`, `:492` | The CSP's connect-src and img-src admit every loopback port, the daemon's CORS is `permissive()`, and `/v1/health` can be read cross-origin. This is pre-existing; §7.8 says "no loosening". → §6 D9. |
| C-25 | `plan-rev3.md:1058`; baselines README row (7); `tauri-runtime-wry-2.11.4/src/lib.rs:4310-4324`; `src-tauri/src/lib.rs:416-427` | Closing the last window quits the app (no RunEvent handler), so "window close/reopen" is quit/relaunch, which Q00 already measured. → Define scenario (7) as quit/relaunch and drop the owed row. |
| D-06 | `modernization-plan.md:377`, `:748-750`; `crates/openalpaca_core/src/config_io.rs:138-141`; `settings_service.rs:36-37`; `apps/openalpaca/src/test_util.rs:150-156` | M17 (fcntl → flock) adds exclusion and blocking within one process, which closes a latent lost-update race. The tree's comments contradict each other about the lock semantics. → Record the race it closes. Audit that no path re-acquires a held lock. Fix the comments. Add one test with concurrent in-process writers, and state the bound. |
| D-09 | `modernization-plan.md:388`, `:580`, `:808-811` | async-trait stays in the lock through zbus on Linux (keyring-core's Secret Service store, opener's `reveal`). A host-default `cargo tree` passes on macOS. → Restate the aim as "no workspace member depends on async-trait". Name zbus and its way out. Run the check with `--target all`. |
| D-10 | `modernization-plan.md:689`, `:745`, `:960` | M11's heading says "Recommended" while §6.4 says Required. M16 leaves its two test-support sites to "if wanted". → Make M11 Required. Include or justify M16's two sites. Add one line saying that "Recommended" in Part III lands under the answers taken. M53 stays Optional; its reason is stated. |
| D-12 | `modernization-plan.md:578-580`, `:804` | LlmProvider has 5 async default bodies, so 10 in total, not 9. dynosaur needs explicit `DynX` constructors, so construction sites change too, and "≈ 89 dyn sites" understates the work. A `futures::Future` spelling is not boxed. → Correct the count. Re-size with construction sites, preferring registration APIs that take `impl Trait`. State the `std::future::Future` spelling rule. |
| D-13 | `modernization-plan.md:452`; M06, M08, M41, M53, M64 | §3.5 item 1's rule for memory code binds these commits, but their entries do not point to it. Their gates already run the memory suites. → Optional: add a pointer, and a `Verified:` line naming the reading. |
| D-15 | `modernization-plan.md:695`; `crates/openalpaca_core/src/prompt_ctx/compaction/graduated.rs:99`; `apps/openalpacad/src/hot_reload.rs:135-138` | M60 cites `context_budget/graduated.rs`, which does not exist. hot_reload.rs:136's refutable `Some(ev) =` arm treats `None` differently once it goes through `run_until_cancelled`. → Fix the path. Record "a closed channel breaks the loop". |
| D-17 | `modernization-plan.md:790`; `crates/openalpaca_core/src/tools/extensions/mod.rs:421-440` | M55 (d) keeps the public `ExtensionSupervisor` trait as "native AFIT", which would draw `async_fn_in_trait`. → Use M13's form: `fn …(&self, …) -> impl Future<Output = T> + Send`. |
| D-19 | `modernization-plan.md:771`, `:988`; `crates/openalpaca_core/tests/memory_eval.rs:43`, `:10208` | M41 and M08 edit the live-eval-gated `memory_eval.rs`, which their per-commit gates compile as an empty crate. V2 catches a break only at the phase end. → Add `cargo test -p openalpaca_core --features live-eval --test memory_eval --no-run` to those commits. |
| E-15 | `modernization-plan.md:739-741`, `:590` | M15's gate keeps option (a)'s line ("reported as the error text, not awaited") beside the line for option (b), which was taken in rev 2.0. (b) copies a private rmcp helper, with no upstream way out named. → Strike or rewrite the (a) line. Name the rmcp gap (no cancellable `call_tool` that runs MRTR) and an rmcp issue as the way out; the owner posts it. |
| E-17 | `tasks/deps-upgrade/README.md:7-8`, `:27`; `modernization-plan.md:3`; `plan-rev3.md:1044` | The index rows name modernization rev 2.0 (M01–M57) and plan-rev3 rev 3.11. `:27` says the archives match no ignore pattern, but `.gitignore:138` ignores them. `modernization-plan.md:3` still says HEAD `109d2ba0`, "rev 3.2" and "Nothing … committed". → Refresh the rows, delete `:27`, and rewrite `:3` as a current status line. |
| E-18 | `tools/gbase/README.md:3`, `:104`; `plan-rev3.md:881` | The self-test has 17 cases, including viii-pins, not 16. P36's body says 65 mechanical items, while the plans say 66. P36 is counted both in Part I's 42 and as M04 + M07 in Part III. CLAUDE.md anchors `:72`/`:168`/`:170` are now `:74`/`:178`/`:180`. → README: 17. Programme total: 42 + 91. Say 65 in M07. Re-cite. |
| F-08 | `tools/e6/typedprobe/Cargo.toml`; `plan-rev3.md:840` | At the consent point, typedprobe loads the real files only through the current build's loaders (toml 0.9). Nothing loads them under toml 1.1 before a live start. → At the same point, also build typedprobe from the prep tree (the wave's manifests), and report index plus ok/err per build. |
| F-12 | `plan-rev3.md:726-729`, `:258` | libsqlite3-sys 0.38.2, the newest (2026-08-08), bundles SQLite 3.53.2, although 3.53.3 (06-26) and 3.53.4 (07-24) fix problems in 3.53.0–3.53.2; 3.54.1 has existed since 10-16. → Name the blocker (no libsqlite3-sys bundles ≥ 3.53.3) and its way out (the next release; vendoring is §6 D11). Read those fixes against our WAL, FTS5 and planner paths. |
| F-13 | `tools/e6/README.md:127-135`; tomlcmp, tedit, typedprobe `main.rs` | E6's proof that nothing was written hashes `config/` only, though the run also reads `plugins/`. Only yamlcmp silences panics; a library panic in the other three would print its default message, which can quote input. → Hash `config/` and `plugins/`. Give the other three a silent panic hook and `catch_unwind`, reported as a `panic-*` word. |
| F-14 | `plan-rev3.md:794`; `crates/openalpaca_mcp/tests/filesystem_e2e.rs:17-27` | P19's real-server gate runs `npx -y @modelcontextprotocol/server-filesystem` unpinned, outside the 7-day rule. → Pin a version at least 7 days old in the test. Use a scratch HOME and `npm_config_cache`. |
| F-15 | `plan-rev3.md:245`, `:756`; `modernization-plan.md:461` | Two records say P05's format fixture is unwritten; it landed as `afb1da60` (`key_encryption/tests.rs:252`). P12's back-out cites keyring 3 reading v4 items, though P12 now moves to keyring-core. → Mark P05 landed. Restate P12's back-out for keyring-core. |
| G-05 | `plan-rev3.md:674`, `:710-714`, `:865`, `:927` | cargo-audit, cargo-deny, zizmor and shellcheck are not installed (`~/.cargo/bin` holds only the rustup proxies), and no step provisions them. → Before the cut, install pinned versions with `cargo install --locked --root <scratch>/tools` (or use P34's CI form). Record the versions in each log. Leave `~/.cargo` untouched. |
| G-06 | `docs/Installation_Manual.md:19`, `:34-38`; `plan-rev3.md:522`, `:1040`, §8 | The Installation Manual is in no §8 row. It promises x86_64 macOS, and gives no Bun version although DU-13 (a) requires Bun ≥ 1.4. Q18 writes the macOS floor into "the release notes", which do not exist. The release scripts check only that bun is present (`package-macos.sh:34`). → Add §8 rows for the manual's Scope and Build Machine Requirements. Add a Bun version check to the package scripts. Have Q18/DU-31 name the manual, or create the release-notes file as a named deliverable. |
| G-07 | `refs/prep/deps-wave-2026-10-12:Cargo.lock` against HEAD | 52 new major versions of transitive crates go unrecorded, some on data paths: cff-parser 0.2 (via pdf-extract); json-patch 4.2 and jsonptr 0.7 (via tauri-build/codegen/utils); typed-path (via zip); atoi_simd and fast-float2 (via calamine). All 70 crates new to the lock carry permissive licences. → Each version-moving commit body lists the new majors it pulls in and from where. Add cff-parser, typed-path and calamine's number parsers to P04's data-path watch-list. |
| G-08 | `.cargo/config.toml:4-5`; `plan-rev3.md:695`; `modernization-plan.md:279`; `tasks/lessons.md:63-66` | P03 deletes `[resolver]` but keeps `[term] color = "always"`, the root cause of the colour-code lesson. → In P03, delete it or set it to `auto`; delete the file if nothing remains; record the choice in R-17. |
| G-09 | `plan-rev3.md:661`, `:930`; `tasks/lessons.md:125-129`; `gbase.sh:630-631` | G-bundle's and E5's `bunx tauri build` runs carry none of the cache confinement the lesson requires. → Set `BUN_INSTALL_CACHE_DIR`, `BUN_RUNTIME_TRANSPILER_CACHE_PATH`, `npm_config_cache` and `npm_config_logs_dir`, and end with the `find -newermt` check, as gbase.sh already does. |

---

## 6. Decisions this review raises for the owner

These are the owner's because each one adds an enforcement point, widens a component, or changes a settled answer (DU-23, DU-2, DU-4, DU-25, DU-27, §7.8). Each comes with a recommendation; none is adopted here. Owner *actions* that are not decisions are listed after them. Anything that touches `~/.openalpaca`, the keychain or a credential is the owner's to do.

**D1 — The cut instant (DU-23 (a)) and the Tauri set (DU-26 (a′) against DU-23).** MF-1, MF-2.

*Recommendation:* cut at or after **2026-10-21T19:53Z (12:53 PDT)**.

- Benefits: it takes the coherent Tauri 2.12.4 family, which Tauri released together, along with its IPC/asset-handler panic fix. It makes rustls 0.23.46 eligible, so D2 becomes moot. It removes the ordered cascade of step-backs.
- Costs:
  - Re-measure the Tauri pair (G-GUI, G-bundle).
  - Re-derive `gbase.sh:341-344` and step 1b's reference lock.
  - Measure the JS releases that become eligible by then: @tanstack/react-query 5.104.2/5.104.3, dompurify 3.4.17, marked 18.1.1, vitest 5.0.4/5.0.5, react 19.3.1.
  - serde-saphyr 2.0.0 becomes eligible before P30 (D4).
- A cut at or after 2026-10-23T04:48Z would also take Rust 1.99.1 at P03 by either clock; D3's P03b reaches the same result without waiting.
- If 20:08Z is kept, P31a, P04 and the driver need ordered, measured step-backs, and D2 needs an answer.
- Either way, settle explicitly whether `:553`'s "the Tauri set is never held" overrides the 7-day rule for Tauri's internals. This review reads it as not overriding.

**D2 — rustls 0.23.46: a DU-23 (c) waiver, or wait.** MF-3. Moot under D1's recommendation.

*Recommendation:* if the cut stays at 20:08Z, waive. It is a same-minor security patch for four advisories, one of which (-0349) is reachable from every HTTPS client in the daemon. Name the ids and their reachability in P04's body. Do not ship 0.23.45 under a silent ignore.

**D3 — Rust 1.99.1: does DU-2's "take an x.y.1" or DU-23's 7-day rule win?** SF-8.

*Recommendation:* DU-23 governs. P03 takes 1.99.0 at the cut, and a one-line P03b `build: Rust 1.99.1` follows through P03's gates once 1.99.1 is eligible. Also state which publication time counts for a toolchain: the channel manifest's 2026-10-15T17:39:23Z, or the GitHub release's 2026-10-16T04:47:44Z. The later one is the conservative reading. Because of the x86_64 miscompilation fix, a waiver is the alternative.

**D4 — serde-saphyr 2.0 at P30 (re-answer DU-4).** SF-9.

*Recommendation:* research first, as §5.1 item 5's own rule says. If the research holds, take the newest eligible 2.x. Otherwise name the blocker and its way out. Do not pin 1.3 silently.

**D5 — P19b presented as a DU-25 sub-row.** E-19.

P19b opens a new outbound `subscriptions/listen` to 2026-07-28 servers, adopted as "the lead's ruling" (`plan-rev3.md:799`). But Part III treats a new outbound protocol message as a decision to present (DU-R4 (v), `modernization-plan.md:590`).

*Recommendation:* take it, since it closes the X-35 silent hole, together with A1-12's one refresh on acknowledgment. Record it in api-fix-plan §0 and CLAUDE.md (SF-18).

**D6 — Refusing MCP redirects across origins.** SF-6.

This adds an enforcement point, and it breaks servers whose configured URL relies on a redirect.

*Recommendation:* take it, as its own `fix(mcp):` before the cut, as §3.3 already recommends. Refuse redirects across origins (same-origin redirects may still be followed), and name the change in `docs/Daemon_Manual.md`.

**D7 — Q04's new refusal for a symlinked ancestor directory.** C-14.

*Recommendation:* keep `std::env::current_exe()` plus canonicalize. That meets G-ident's aim without the ancestor rule. If Tauri's form is wanted, record the refusal and name the `process-relaunch-dangerous-allow-symlink-macos` feature as its way out.

**D8 — Reveal as a daemon route by file id (DU-27 option (e)).** C-13.

This changes §7.8's settled rule of "no daemon API change beyond the additive protocol field", and it widens the daemon.

*No recommendation to adopt it by default.* It deserves one answer before Q06, because it would remove Q06's ≈1,070-line move and the webview-supplied paths, while keeping Q05 in the shell.

**D9 — The loopback wildcard in the CSP and the daemon's permissive CORS.** C-24.

*Recommendation:* record it in `tasks/pending-decisions.md` as accepted residue, with its reason (the daemon's port is dynamic). Revisit narrowing — a runtime-pinned connect-src, or daemon CORS restricted to the Tauri origins plus the dev origin — once Q04 lands.

**D10 — P28 and P41: wait for test-bot tokens, or waive their live gates.** SF-12.

*Recommendation:* wait. The rule names the token as P28's blocker. The owner creates a Telegram and a Discord test bot.

**D11 — SQLite: vendor an amalgamation at 3.53.4 or later, or wait for libsqlite3-sys.** F-12.

*Recommendation:* wait, with the blocker named in P06. Vendoring would change the contract of the bundled build.

**D12 — DU-13 (b): reinstall a Bun 1.3.x, or record the fallback as needing a fresh install.** SF-29.

*Recommendation:* record that (b) needs a fresh 1.3.x installed in its own `BUN_INSTALL` root at the time it triggers. dependabot-core reads lockfile versions up to 3, so the fallback is unlikely to be needed.

**D13 — Q00's owed GUI rows.** SF-5.

*Recommendation:* do not hold the cut for them. Record that they will be taken later, on a rebuilt `a91f4b23` bundle, under MF-4's corrected launcher.

**Owner actions** (no decision needed):

- Push the branch and open a draft PR, with a second CI read after P12/P13 (SF-2).
- Take the backup immediately before the first bumped start, and rehearse the restore (SF-1).
- Create the test-bot tokens (SF-12).
- File the pdf-extract issue with the two fixtures (A1-16).
- Install a Bun 1.3.x if D12 is answered the other way (SF-29).

---

## 7. What was verified correct, and what was not verified

### Verified correct

From the critic's checks and the verifiers' corrections:

**Toolchain, data and dependencies**

- **Unicode tables do not change across the toolchain move.** `char::UNICODE_VERSION` is (17, 0, 0) under both rustc 1.93.0 and 1.99.0, and unicode-normalization is 0.1.25 in both HEAD's lock and the prep lock. So project folder names (`store/projects.rs:149-151`), note keys (`memory_files/note.rs:312`) and the memory index's normalization (`memory_index/query.rs:226-247`) give the same output, and P03 owes no `SCHEMA_VERSION` bump.
- **The app's local storage survives the Tauri pair commit.** The WKWebView data store and origin are unchanged from Tauri 2.11.5/wry 0.55.1 to 2.12.3/wry 0.57.0 on macOS: `defaultDataStore` is still used, the origin is still `tauri://localhost`, and no `dataStoreIdentifier` is set. So pane widths, pins and `oa-project` survive.
- **The YAML scope is complete.** Agent templates are parsed by a hand parser (`agent/template/mod.rs:171`), and serde_yaml is used only in `middleware/skill/`, so P30's and yamlcmp's SKILL.md-only scope covers everything. E6 includes plugin-contributed SKILL.md files.
- **`tests/common/eval.rs` (P10's site `:445`) is compiled under `--all-targets` without live-eval,** through `memory_eval_corpus.rs:22-23`. Only `memory_eval.rs` is gated behind `live-eval`.
- **Licences are clean.** All 70 crates new to the prep lock are permissive (MIT and/or Apache-2.0, or Zlib). keyring 3 and dbus-secret-service are gone, and secret-service enters only through zbus-secret-service-keyring-store, on Linux.
- **The driver's JS set is fresh.** At 2026-10-17T20:08Z it is the newest eligible for every pin (`gbase.sh:598-616`). The first newer eligible JS release is react-query 5.104.2, at 10-18T19:35Z.
- **The driver needs no newer Python.** lockdiff.py and lockdates.py use only stdlib modules present in the system Python 3.9.6.
- **Recorded elsewhere, not hidden.**
  - Vite's explicit `build.target: "es2022"` (`vite.config.ts:34`) is kept by P32 and handled by Q18.
  - M18's difference in Windows store location is recorded with its re-examination trigger (`modernization-plan.md:378`).

**Points where the plan was already right**

- **The step-back rule exists.** P04's text has it (`plan-rev3.md:710`); what is missing is enforcement and a current table (MF-2).
- **The JS half of DU-23 is enforced** by bunfig.toml (`minimumReleaseAge = 604800`), and `tauri-plugin-dialog = "=2.8.2"` holds dialog 2.8.3 back.
- **The MCP redirect leak is recorded** (§3.3 `:449`, `tasks/pending-decisions.md:251`); only a scheduled commit is missing.
- **§5.1 item 5 already has a rule for new majors**, and §5.2's temp-store rule exists; both lack only follow-through.
- **The evidence tarballs are git-ignored** (`.gitignore:138-139`), so archived logs do not enter git history.
- **G-base step 8's `cargo check --workspace --all-targets`** catches type errors in test targets before the final phase.
- **CI triggers only on a PR or a push to main,** so no position for Q08 could give coverage per commit. G-GUI by hand is attached by name to the shell commits.
- **Several citations still hold:** `router.rs:480` (`.layer(cors_layer())`), and the citations anchored "at `865a2376`" (P12's `:155`, P16's `:153`).
- **The memory doctor resolves recorded roots as written first** (`store/projects.rs:426-445`). It does not canonicalize the real project roots in the normal case.
- **P36's 100 → 0 plausibly holds without M08.** `manual_let_else` is a pedantic lint and no workspace lint configuration enables it. The record should still say which tip it was measured on.
- **Two holds are still valid:** zip 8.6 (under calamine) and lopdf 0.42 (against 0.45).
- **E8 must not use `immutable=1`.** It would skip the copied WAL and undercount rows.

### Not verified

**No build, install or rebase was run.**

- Every compile claim rests on offline lock resolution or on `cargo tree`: whether tauri 2.12.3 compiles against the 10-14 internals, the step-back cascade, and C-15.
- No `bun install` was run. Not checked:
  - the GUI suite under Vite 8 / Vitest 5 / jsdom 30.1 / React 19.3;
  - Bun 1.4.3's v1/v2 lock behaviour (measured for 1.4.2 only);
  - whether Bun 1.3.x reads a v2 lock;
  - how minimumReleaseAge treats @tauri-apps/cli's exact-pinned platform binaries;
  - P33's dist byte identity and the Lightning CSS output.
- No rebase of the prep deltas was attempted; E-02's conflict risk is inferred.

**Advisories and registries.**

- cargo-audit and cargo-deny were not run locally. Advisories were matched by script against advisory-db `64f92cd`; RustSec and npm imports after about 2026-10-17T01:30Z are unknown.
- GitHub API rate limits cut several reads: the freshness of the actions' SHAs, the ranges of vite 7.3.6's advisories, and the RustCrypto, keyring and aws-lc-rs advisory lists.
- B-03's list of about 33 step-backs ignores MSRV, feature and target filters.
- The other new advisories E2 names (negentropy, pest_grammars and others) were not cross-checked exhaustively against the final lock.

**Hosted services.**

- The Dependabot bun updater's image.
- Whether it accepts `packageManager` 1.4.3 while bundling 1.4.2.
- Whether GitHub validates dependabot.yml from a non-default branch.
- CI runner headroom after the wave (the rust job's 45-minute timeout), and the repository's visibility.

**Needs network, a daemon or the owner's machine.**

- P19 and P19b against real MCP servers, and the forking-fixture descendant check.
- P41's live Discord gate.
- E7's `OPENALPACA_DAEMON_BIN` wrapper with the pre-wave CLI.
- DU-10's count and E6 on the real files.
- The keychain's ACL behaviour with a re-signed binary.
- Whether a Bun 1.3.x exists elsewhere on the owner's machine (only `~/.bun*` was listed).
- Whether the owner's PATH holds an `openalpacad`.
- Whether Apple's `/usr/bin/sqlite3` 3.54.0 is built with FTS5.
- A full audit of test entry points that boot `initialize_services` on a fresh store (only spot-read).

**Linux and Windows** (deferred by DU-33).

- Any Linux or Windows compile: zbus with crypto-rust, P13.
- Linux and Windows warning counts, and whether M39's `#[expect]`s are fulfilled on Linux. G-02 rests on reading the source.
- keyring-core's LazyLock caching a failed Secret Service connection on Linux.
- P27a's Linux Open.
- Rollback of P03 after P31a/P04 (the new lock's MSRVs), and of P31's v2 bun.lock under an older Bun.

**The packaged app.**

- M70 and M71 in WKWebView.
- Every G-bundle leg (M72–M79).
- M73's style-src and nonce.
- M76's `on_download` path in wry 0.57.
- Whether WebKit's XPC helpers inherit a profile beyond what Q00 observed (Q00's evidence tarball was not opened).
- Whether `sandbox-exec` runs on macos-latest.
- `/v1/health` latency under load (C-09 is reasoned, not measured).

**Release notes and changelogs not re-read.**

- SQLite: P06's 3.52/3.53 claims, and the contents of 3.53.3 and 3.53.4.
- serde_json 1.0.152 → 1.0.154 formatting.
- aws-lc-rs 1.18.2's AWS-LC notes, and wry 0.57.0's date.
- The runtime behaviour of rustyline 18, opener 0.9 and tower-http 0.7.
- Cargo 1.99/1.100 claims in §9 (the Cargo Book changelog has moved).
- Whether Rust 1.99.1's x86_64 FFI-bool fix or #163947 touches this tree.

**Read only in part.**

- The G-base driver beyond the cited functions, and prep P19b's code.
- P36 and M08 by file list and commit body only; the 1585/1586 fmt difference was not traced.
- M43's per-site semantics.
- M69's Babel claims.
- M15 and M84's rmcp MRTR/progress details.
- M65 together with future-incompatibility reports.
- M07's measurements, taken from the prep messages.
- `#[source]` on `anyhow::Error` under thiserror 2.0.20 (not compiled).
- The shell's own lint counts.
- plan-rev3 §2.12, §2.13, §7.8 and Appendix A, and modernization §1.
- The docs manuals: Daemon_Manual's MCP paragraph, and the API_MAP rows Q04 cites.
- tools/e6's tedit target pin.
- Whether lockdates/lockdiff can take an age threshold without changing their exit contract (header only).
- The Q05/Q06 reference recount; G8's cost and Q07's line estimates; serde-saphyr 2.0's option names.
- `tasks/lessons.md` beyond the cited lines, and the Obsidian ADR-048/049 entries (outside the repository).
