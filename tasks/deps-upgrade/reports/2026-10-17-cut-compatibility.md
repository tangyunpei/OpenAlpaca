# Cut-day compatibility — the package set at 2026-10-21T19:53Z (2026-10-17)

**Status: rev 2** — rev 1 measured on 2026-10-17T05:50Z (2026-10-16 22:50 PDT); base `refs/prep/deps-wave-2026-10-12` (`7e702582`); eligibility at or before **2026-10-14T19:53:00Z** (the cut minus 7 days, DU-23); Rust 1.99.0, Bun 1.4.3. Owner decisions in force: **D1 (a)** (the cut at or after 2026-10-21T19:53Z, 12:53 PDT), **D2 (a)** (moot: rustls 0.23.46 is eligible), **D3 (a)** (P03 takes Rust 1.99.0; a later P03b takes 1.99.1). **D4** (serde-saphyr 2.0 at P30) is still the owner's; §5 is evidence for it, not a decision. Answers the owner's request "just make sure all packages are compatible". Read against `tasks/deps-upgrade/plan-rev3.md` **rev 3.13** and the G-base driver as committed at HEAD `ff0e7740`; the working tree's uncommitted rev 3.14 (another workflow's ZCode-audit pass) mentions every version literal cited here exactly as often as rev 3.13 does, so nothing below depends on which one is read. **Rev 2** (2026-10-17) adds §8, one tree carrying both halves — the prep tip with the Rust leg's lock, P03, `=2.8.3`, P27a's opener 0.9 and the whole new JS set — measured from 07:28:41Z (00:28:41 PDT) on the same base, Rust 1.99.0 and Bun 1.4.3; it builds and passes every gate run on it, a debug app bundle and G-ident's debug variant included, while the workspace suite, check and clippy were not run on its exact lock and nothing was launched. §1, C3, §6.3's note, §7 and the evidence note below are updated to it. The re-anchoring §6 asks for is written, in the same step, by plan-rev3 rev 3.15, modernization-plan rev 2.4, the G-base driver's defaults (its README's status line and 2026-10-17 revision-log rows) and the records (`tasks/pending-decisions.md` rev 6, api-fix-plan rev 3.29).

| rev | date | change |
|---|---|---|
| 1 | 2026-10-17 | First version: four measurement legs (Rust, JS, registry, serde-saphyr) and one verifier, run read-only on the repository in the session scratchpad. |
| 2 | 2026-10-17 | **§8, the combined tree**: the two halves in one worktree of the prep tip (the Rust leg's lock with P27a's opener 0.9, which makes it rev 1's opener variant byte for byte; the JS leg's pins and `bun.lock` byte for byte), measured from 07:28:41Z — the lock and freshness checks, C5's `bun.lock` check, the four bun gates, G-GUI, a debug `tauri build --bundles app`, G-ident's debug variant and the pair, all passing; its findings (§8.3): tauri-cli 2.12.3 refuses `CI=1`, `cargo update -p opener` re-points 21 dependency edges, the CLI's npm version check checks nothing under Bun, and where Vitest 5.0.4 writes. §1's verdict, first qualification, counts and overall line, C3's lock-move sentence, a note under §6.3 (the driver's defaults are re-anchored), §7 (the combined-tree, runtime, opener, Rust 1.99.1 and Vitest items) and the evidence note (rev 1's evidence is archived; rev 1 said it was not) updated to it. Rev 1's measurements are unchanged. |

## How this was measured

Because no release published after 2026-10-14T19:53:00Z can be eligible at a 2026-10-21T19:53Z cut, the whole cut-day eligible set already exists and was measured exactly today. Four legs and a verifier ran on 2026-10-17 from 05:46Z (2026-10-16 22:46 PDT):

- **The Rust leg** worked in a detached worktree of the prep tip `7e702582` — the measured re-cut of every planned Rust move as of 2026-10-12 (the Tauri 2.12.3 pair included) plus the pre-written code commits (keychain-safe credential discovery, P10, P12, P15–P19b, P41, P36, M08). On it: `.cargo/config.toml` deleted (P03, rev 3.9 / G-08), `tauri-plugin-dialog = "=2.8.3"`, then `cargo +1.99.0 update` over the whole lock and the freshness gate's step-back loop until it passed. Then every Rust gate, G-GUI and two scratch variants (opener 0.9, serde-saphyr 2.0). The resulting tree is the prep tip plus three uncommitted changes, saved as `rust/logs/90-worktree-diff.patch`; the lock is `rust/locks/final.lock`.
- **The JS leg** worked in a second detached worktree of the prep tip (whose JS set is HEAD's): the 19 `package.json` pin changes of P31/P32 and P31a's JS half, P33's plugin swap (with the one-line `vite.config.ts` import change), root `packageManager: bun@1.4.3` (P42), `rm bun.lock` and a from-scratch `bun install` under a scratch `bunfig.toml` whose `minimumReleaseAge` (208,862 s) put the age limit exactly at the max date. Then the four bun gates, peers, engines, audit, the CI-shaped `--frozen-lockfile` run under the committed bunfig (604,800 s), and P31a's Rust half in scratch with G-GUI on it, the Vite 8 `dist/` embedded.
- **The registry leg** built nothing: crates.io's sparse index and API, the npm registry, RustSec's advisory database at `64f92cd` (its HEAD at 06:09Z), GitHub release notes and repository advisories, `.crate` and npm tarballs opened as data only.
- **The serde-saphyr leg** worked in a third worktree of the prep tip: 2.0 as a drop-in and migrated, a scratch three-way comparator (serde_yaml 0.9.34, serde-saphyr 1.3.0 and 2.0.0 in one binary) over the four repository `SKILL.md` files and 42 synthetic frontmatters, and E6's `yamlcmp` rebuilt against 2.0.
- **The verifier** re-ran HEAD's `lockdates.py` with a fresh cache against the Rust leg's final lock and every `bun.lock` entry, checked yanks, measured the rustfmt drift on the unmodified prep tip under both toolchains, and compared the legs with each other.

Every check used HEAD's `tasks/deps-upgrade/tools/gbase/lockdates.py` (it carries `dd677009`'s gate; the legs' copies are byte-identical to it), scratch `CARGO_TARGET_DIR`, `HOME`, Bun install root and store. No daemon, app, `tauri dev` or `tauri build` was started; nothing touched `~/.openalpaca` or the keychain; `~/.bun`, `~/Library/Caches/bun`, `~/.npm/_logs`, `~/.cargo/bin` and `~/.rustup` gained nothing (`rust/logs/92-home-writes-check.log`, `js/logs/99-home-writes.log`, `registry/logs/35-no-writes-outside-scratch.log`, `saphyr/logs/45-closeout.log`, `verify/logs/04-home-writes.log`). All three worktrees were removed; the repository's HEAD is still `ff0e7740`. Its working-tree edits (among them `tasks/deps-upgrade/tools/e6/yamlcmp/src/main.rs`) belong to another workflow, so this report cites HEAD's lines.

**Rev 1's evidence is archived** as `tasks/deps-upgrade/evidence-2026-10-17-compat.tar.gz` (git-ignored, written with `6d3e47fc`; its `compat/` tree is `$S` below; rev 1 said it was not archived). §8's evidence is in the session scratchpad (`$C`, §8). Every rev 1 log path below is relative to

`S = /private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/4b1ee495-f087-45a9-9fa8-4325993552c8/scratchpad/compat`

(`$S/rust/`, `$S/js/`, `$S/registry/`, `$S/saphyr/`, `$S/verify/`). A log whose name ends in `.txt` is a derived analysis with no command header; the others open with their command. The scratchpad lives only as long as this session, which is why rev 1's tree was archived.

---

## 1. Verdict

**Yes: every package in the cut-day set is compatible.** The newest set eligible at D1 (a)'s cut builds, lints clean and passes every test on Rust 1.99.0 and Bun 1.4.3, with no source change beyond P33's planned one-line import swap — leg by leg in rev 1, and in rev 2 in one tree carrying both halves, where every gate run on it passes: the four bun gates, G-GUI, a debug app bundle whose daemon is the staged one byte for byte, and the pair (§8). The Tauri pair holds across both registries, every peer and engine range is met, every lock entry is dated at or before the max date, nothing is yanked or a pre-release, and no crate needs a Rust newer than 1.96.

Three things qualify that answer, and they are what must change for the wave to land compatible:

1. **One tree has carried both halves — in scratch, and not every gate ran on it** (rev 2, §8; rev 1 read "no single tree carried both halves"). In rev 1 the Rust leg's tree had HEAD's JS set and the JS leg's tree only the Tauri `cargo update` on the prep lock; the 15 Tauri-family lock entries are identical in the two (verifier). §8's combined tree — the prep tip with the Rust leg's lock, P03, `=2.8.3`, P27a's opener 0.9 and the whole new JS set — passes the lock and freshness checks, C5's `bun.lock` check, the four bun gates (121 files / 1,551 tests), G-GUI (0 / 0 / 0), a debug `tauri build` whose bundled daemon equals the staged sidecar and cargo's artifact (G-ident's debug variant), and the pair. Not on it: the workspace suite, check and clippy (rev 1's 5,126 / 0 / 17 and clippy 0 are of the lock without opener 0.9), any launch, a release bundle. The G-base run at the cut and the wave's own gates remain the gates of record.
2. **The prep tip's P30, as pre-written, refuses some skill files that HEAD reads.** serde-saphyr 1.3 — and 2.0 with its default features — refuses a `SKILL.md` whose frontmatter has 33 or more consecutive comment lines between a key and its block value (a commented-out block under `routing:` is the typical case); HEAD's serde_yaml 0.9 reads it. This does not depend on D4. The fix is one option, `emit_comments: false`, plus a pin test (§4, C1). None of the four repository skills hits it; whether any of the owner's real skills does is unknown until E6 runs.
3. **The prep tip is stale against the cut set.** It pins `tauri-plugin-dialog = "=2.8.2"` (the cut needs `=2.8.3`, the npm twin), keeps `opener = "0.8"` although P27a is planned, and writes floors below MF-2's rule (rustls, sha2, keyring-core, zip). From a lock already on the 2.12.3 family, P31a's planned `cargo update -p tauri -p tauri-build -p tauri-plugin-dialog` leaves a mixed Tauri family that was never released together; from HEAD's lock — the branch's real base — the same command takes the whole 2026-10-14 family at once (§4, C2, C3). The Rust leg's whole-lock `cargo update` hid this.

Beside those, the plan's text needs re-anchoring (§6): six JS literals and the driver's defaults, P04's step-back list, a freshness check for `bun.lock`, the three zip 8.6.0 advisories carried as reasoned ignores (no eligible release fixes them), and two new transitive entries named. **Overall: compatible, with fixes** — the verifier's verdict; rev 2's combined tree builds and passes every gate run on it (§8).

**The counts.**

| | |
|---|---|
| Rust lock | 765 packages: 754 from crates.io, 11 workspace members. All 754 dated at or before the max date (the Rust leg and, independently, the verifier); 0 yanked, 0 pre-release, 0 non-crates.io sources; highest declared rust-version 1.96 (takecell). Against the prep lock: 25 moved, 1 added (`static_vcruntime 3.0.0`), 0 removed. Against HEAD's: 192 moved, 69 added, 31 removed. |
| Step-backs | 11 `--precise` steps over 13 releases younger than the max date; **none is a Tauri crate**. |
| Rust gates | check 0 warnings; clippy 0 (file, line, lint) triples and 0 `deprecated`; tests **5,126 passed / 0 failed / 17 ignored**; live-eval clippy 0; G-GUI check 0 warnings, clippy 0 triples, fmt 0 hunks. |
| JS lock | `lockfileVersion: 2`, 219 entries (218 unique name@version pairs, 204 names); 0 younger than the max date (the JS leg and, independently, the verifier). |
| JS gates | `bun install` exit 0, no warnings; `check`, `format:check`, `build` exit 0; tests **121 files / 1,551 tests**; 31 peer edges, 0 unmet; 81 `engines.node` ranges, all met at Node 24.15.0 and 22.23.3; `bun audit` 0 vulnerabilities in 203 packages. |
| Advisories (RustSec) | 3 vulnerabilities left, all zip 8.6.0 through calamine 0.36.1 (extract paths we never call); the prep lock has 7, HEAD's 14. |
| Combined tree (rev 2, §8) | One worktree with both halves: `Cargo.lock` = rev 1's opener variant (765 packages; 754 dated, all eligible), `bun.lock` = the JS leg's (218 pairs, all eligible), both byte for byte; the four bun gates (121 files / 1,551 tests), G-GUI check, clippy and fmt 0, a debug app bundle with G-ident's debug variant, and the pair pass. Not run on it: the workspace suite, check and clippy; any launch; a release bundle. |

---

## 2. The cut-day set

**Marks.** `=` the plan's current text names this version. `†` the plan says "the newest eligible at the cut instant", and its rev 3.13 forward text already names this value at D1's instant — re-anchoring writes it in as the target. `▲` the plan names an older version, or none — re-anchoring changes it. "Prep tip" gives the version on `7e702582` where it differs (for JS, that is HEAD's version). Dates are crates.io `created_at` or npm `time`, UTC.

### 2.1 The toolchain

| Item | Plan's text | Cut-day | Published | Mark | Prep tip | Note |
|---|---|---|---|---|---|---|
| Rust (rustc, cargo, clippy, rustfmt) | P03: 1.99.0 | **1.99.0** | 2026-10-01T12:41:12Z (GitHub release; channel manifest 12:38:04Z) | = | 1.99.0 | 1.99.1 (channel manifest 2026-10-15T17:39:23Z, GitHub release 2026-10-16T04:47:44Z) is not eligible; D3 (a) moves to it in P03b. `rustc +1.99.0 -V`: `b940084d7 2026-09-28`. |
| workspace `rust-version` | 1.99 | 1.99 | — | = | 1.99 | |
| Bun | P42: 1.4.3 | **1.4.3** | 2026-10-10T05:24:54Z (npm `bun` 05:31:17.717Z) | = | 1.4.2 (HEAD's pin) | The newest 1.4.x. Every JS gate ran under `1.4.3+c6da4a4d3`. |
| Node | unpinned | local 24.15.0; CI default 22.23.3 (the plan's dated record) | — | = | — | jsdom 30.1.2 needs `^22.22.2 \|\| ^24.15.0 \|\| >=26`: the local Node sits exactly on its floor. |

### 2.2 The Tauri family and the pair

Margin = the max date (19:53:00Z) minus the publication time.

| Crate or package | Plan's text | Cut-day | Published | Margin | Mark | Prep tip |
|---|---|---|---|---|---|---|
| `tauri` | P31a `~2.12`; the 2026-10-14 family at D1's instant | **2.12.4** | 2026-10-14T18:52:10Z | 1 h 00 m 50 s | † | 2.12.3 |
| `tauri-build` | `~2.7` | **2.7.3** | 2026-10-14T18:51:08Z | 1 h 01 m 52 s | † | 2.7.2 |
| `tauri-codegen` | with tauri | 2.7.3 | 2026-10-14T18:50:09Z | 1 h 02 m 51 s | † | 2.7.2 |
| `tauri-macros` | with tauri | 2.7.3 | 2026-10-14T18:50:40Z | 1 h 02 m 20 s | † | 2.7.2 |
| `tauri-runtime` | with tauri | 2.12.2 | 2026-10-14T18:49:28Z | 1 h 03 m 32 s | † | 2.12.1 |
| `tauri-runtime-wry` | with tauri | 2.12.2 | 2026-10-14T18:49:41Z | 1 h 03 m 19 s | † | 2.12.1 |
| `tauri-utils` | with tauri | 2.10.2 | 2026-10-14T18:47:14Z | 1 h 05 m 46 s | † | 2.10.1 |
| `tauri-plugin` | with tauri | 2.7.2 | 2026-10-14T18:50:53Z | 1 h 02 m 07 s | † | 2.7.1 |
| `tauri-plugin-fs` | with the dialog plugin | 2.7.1 | 2026-10-14T19:15:53Z | 37 m 07 s | † | 2.7.0 |
| `tauri-plugin-dialog` | `=2.8.3` at D1's instant | **2.8.3** | 2026-10-14T19:16:07Z | 36 m 53 s | † | 2.8.2 (`=2.8.2`) |
| `wry` / `tao` / `swift-rs` | (dates were [NE]) | 0.57.0 / 0.37.1 / 1.0.9 | 2026-09-08T03:54:05Z / 2026-09-26T10:49:56Z / 2026-10-10T10:41:32Z | — | = | same |
| `static_vcruntime` (new, via tauri-build 2.7.3; Windows only) | not named | 3.0.0 | 2025-11-25T15:38:25Z | — | ▲ | absent |
| `tauri-winres` | not named | 0.3.6 | 2026-04-27T03:06:53Z | — | = | same |
| `@tauri-apps/api` | 2.12.4 at D1's instant | **2.12.4** | 2026-10-14T18:47:44.921Z | 1 h 05 m 15.079 s | † | 2.11.1 |
| `@tauri-apps/plugin-dialog` | 2.8.3 at D1's instant | **2.8.3** | 2026-10-14T19:17:55.842Z | 35 m 04.158 s | † | 2.7.3 |
| `@tauri-apps/cli` | 2.12.3 at D1's instant | **2.12.3** | 2026-10-14T19:50:25.524Z | 2 m 34.476 s | † | 2.11.4 |
| `@tauri-apps/cli-linux-riscv64-gnu` (the last of its 11 platform binaries) | — | 2.12.3 | 2026-10-14T19:52:53.494Z | **6.506 s** | — | — |

**The pair holds:** `tauri` 2.12.4 and `@tauri-apps/api` 2.12.4 share a minor; `tauri-plugin-dialog` 2.8.3 equals `@tauri-apps/plugin-dialog` 2.8.3 exactly; `@tauri-apps/cli` 2.12.3 is on 2.12; each lock holds one copy of each (`js/logs/40-pair-check.log`). The 6.506 s row is the one that flips if the instant moves earlier.

### 2.3 Rust direct dependencies

**The ones the wave moves, or whose cut-day version differs from the prep lock:**

| Crate | Commit | Plan's text | Cut-day | Published | Mark | Prep tip | Note |
|---|---|---|---|---|---|---|---|
| rusqlite (libsqlite3-sys 0.38.2) | P06 | 0.40.2 | 0.40.2 | 2026-08-08T14:21:52Z | = | same | No libsqlite3-sys yet bundles SQLite ≥ 3.53.3 (D11). |
| lru | P07 | 0.18.5 | 0.18.5 | 2026-09-23T02:33:10Z | = | same | |
| sysinfo | P08 | 0.39.6 | 0.39.6 | 2026-07-09T10:46:59Z | = | same | rust-version 1.95. |
| base64 | P09 | 0.23.1 | 0.23.1 | 2026-08-04T12:34:31Z | = | same | |
| sha2 | P10 | floor 0.11.1 (MF-2) | **0.11.1** | 2026-10-12T18:58:22Z | † | `"0.11"`, lock 0.11.0 | rust-version 1.85 → 1.88; an AArch64 round-constant fix. |
| aes-gcm | P11 | 0.11.1 | 0.11.1 | 2026-08-21T20:55:46Z | = | same | |
| keyring-core | P12 | floor 1.0.1 (MF-2) | **1.0.1** | 2026-10-13T00:02:42Z | † | `"1.0"`, lock 1.0.0 | No functional change (tests, CI). |
| apple- / windows- / zbus-secret-service-keyring-store | P12 | 1.0.2 / 1.1 / 1.0.1 | 1.0.2 / 1.1.0 / 1.0.1 | 2026-08-06 / 2026-05-24 / 2026-08-15 | = | same | Each the newest. |
| pdf-extract (lopdf 0.42.0) | P15 | 0.12.1 | 0.12.1 | 2026-09-16T15:12:00Z | = | same | Brings ttf-parser 0.25.1 (unmaintained, informational). |
| calamine | P16 | 0.36 | 0.36.1 | 2026-07-27T10:06:39Z | = | same | The newest release; requires `zip ^8.6`. |
| zip (ours) | P16 | 9.0.3 at D1's instant | **9.0.3** | 2026-10-13T19:55:20Z | † | `"9.0.2"`, lock 9.0.2 | 9.0.4 (2026-10-16T09:09:57Z) too young. calamine keeps its own zip 8.6.0. |
| infer | P18 | 0.22.0 | 0.22.0 | 2026-07-15T15:05:03Z | = | same | |
| rmcp (+ rmcp-macros) | P19 | 3.5.1 | 3.5.1 | 2026-10-05T18:35:06Z | = | same | No 3.5.2 exists. |
| process-wrap | P19 | 10.0.1 | 10.0.1 | 2026-09-23T05:26:52Z | = | same | |
| tower-http | P25 | 0.7.1 | 0.7.1 | 2026-08-31T14:49:25Z | = | same | 0.6 stays beside it for reqwest. |
| tokio-tungstenite | P26 | 0.30.0 | 0.30.0 | 2026-07-11T12:58:44Z | = | same | |
| rustyline | P27 | 18.0.1 | 18.0.1 | 2026-06-24T17:20:34Z | = | same | |
| opener | P27a | `"0.9"` (0.9.0) | **0.9.0** | 2026-09-29T03:32:47Z | = | **`"0.8"`, lock 0.8.5** | Not in the prep tip (§4, C3). |
| toml | P29 | `"1.1"`; 1.1.8 eligible by the cut (P04) | 1.1.8+spec-1.1.0 | 2026-10-09T02:46:31Z | † | same | |
| toml_edit | P04 | `"0.25"`; 0.25.17 (P04) | 0.25.17+spec-1.1.0 | 2026-10-09T02:46:32Z | † | same | DU-23 (b)'s step-backs are moot. |
| serde-saphyr | P30 | 1.3.0, pending D4 | 1.3.0 | 2026-09-16T16:41:36Z | = | same | 2.0.0 (2026-10-13T15:11:53Z) is eligible; §5. |
| rustls | P04 / P41 | ≥ 0.23.46 (P04's gate) | **0.23.46** | 2026-10-14T09:54:15Z | † | `"0.23"`, lock 0.23.45 | Clears RUSTSEC-2026-0346…-0349; D2 moot. No API, feature or rust-version change. |
| aws-lc-rs / aws-lc-sys (transitive) | P04 | not named | 1.18.2 / **0.46.0** | 2026-10-12T15:05:29Z / 15:02:50Z | ▲ | 1.18.1 / 0.45.0 | A new 0.x minor of aws-lc-sys; bundled AWS-LC 5.7.0 → 5.11.0, which rejects non-canonical Ed25519 public keys (§4, C9). |
| tokio | P04 | 1.53.2 | **1.53.3** | 2026-10-13T20:51:46Z | ▲ | 1.53.2 | Deprecates `AsyncFd::new` and siblings; 0 uses in the tree (`registry/logs/36-grep-asyncfd.log`). |
| serde_json | P04 | 1.0.154 eligible after 10-19 | 1.0.154 | 2026-10-11T15:31:46Z | † | same | |
| uuid | P04 | 1.28.0 | 1.28.0 | 2026-10-10T00:02:40Z | † | same | |
| tokio-util | P04 | 0.7.20 | 0.7.20 | 2026-10-09T11:02:11Z | † | same | |
| signal-hook | P04 | 0.4.5 | 0.4.5 | 2026-10-04T13:50:34Z | † | same | |
| libc | P04 | 0.2.190 | 0.2.190 | 2026-10-02T19:33:19Z | = | same | Stepped back from 0.2.191 (2026-10-16T22:56:46Z). |
| unicode-normalization | — | 0.1.25 | 0.1.25 | 2025-10-30T16:06:38Z | = | same | Stepped back from 0.1.26 (2026-10-16T23:28:29Z); on the data-path watch-list. |
| clap / console / rand / reqwest / thiserror | P04 | 4.6.7 / 0.16.6 / 0.10.3 / 0.13.5 / 2.0.21 | same | 2026-09-14 / 09-10 / 09-20 / 09-08 / 09-23 | = | same | |

**Unchanged, each the newest eligible within its requirement:** anyhow 1.0.104 (2026-07-18), arc-swap 1.9.2 (2026-06-28), async-trait 0.1.92 (2026-08-08), axum 0.8.9 (2026-04-14), blake3 1.8.7 (2026-08-20), bytes 1.12.1 (2026-07-08), chrono 0.4.45 (2026-06-04), colored 3.1.1 (2026-01-16), dashmap 6.2.1 (2026-05-17), dialoguer 0.12.0 (2025-08-23), directories 6.0.0 (2025-01-12), file-lock 2.1.11 (2024-02-17), futures-util 0.3.34 (2026-08-11), mime_guess 2.0.5 (2024-06-29), nix 0.31.3 (2026-05-11), notify 8.2.0 (2025-08-03), regex 1.13.1 (2026-07-15), serde 1.0.229 (2026-07-18), shell-escape 0.1.5 (2020-06-19), similar 3.2.0 (2026-08-17), teloxide 0.17.0 (2025-07-11), tempfile 3.27.0 (2026-03-11), tokio-cron-scheduler 0.15.1 (2025-10-28), tokio-stream 0.1.19 (2026-07-22), tower 0.5.3 (2026-01-12, dev), tracing 0.1.44 (2025-12-18), tracing-subscriber 0.3.23 (2026-03-13), tracing-test 0.2.6 (2026-02-07, dev), twilight-gateway / -http / -model 0.17.1 (2025-12-13), url 2.5.8 (2026-01-05), urlencoding 2.1.3 (2023-07-21), which 8.0.6 (2026-08-26). Source: `rust/logs/18-direct-deps-table.log`, `rust/logs/17-newest-eligible.log`.

**Newer eligible majors of direct dependencies:** two — opener 0.9.0 (planned, P27a) and serde-saphyr 2.0.0 (D4). **Newer eligible releases held inside their range by an upstream pin** (each named with its holder, `rust/logs/17b-held-in-range-holders.txt`): `matchit =0.8.4` by axum 0.8.9; `generic-array =0.14.7` by crypto-common 0.1.7; `brotli-decompressor ~6.0` by brotli 9.0.0 (Tauri); `toml 0.8.2` / `toml_datetime 0.6.3` / `toml_edit 0.20.2` by proc-macro-crate 2.0.2's `=` pins through glib-macros 0.18 (Linux only). The way out of each is the holder's next release.

**Part III additions, registry facts only (not built):** dynosaur 0.3.1 (2026-07-03, rust-version 1.84, M58); jsonschema 0.58.6 (2026-10-06; 0.59.0 of 2026-10-16 too young; its required dependencies resolve to eligible releases with no RustSec match, `registry/logs/32-part3-newdeps-resolved.log`; M82).

### 2.4 The JS packages

| Package | Commit | Plan's text | Cut-day | Published | Mark | HEAD / prep tip | Note |
|---|---|---|---|---|---|---|---|
| react | P31 | 19.3.0 measured; 19.3.1 at D1 (rev 3.13) | **19.3.1** | 2026-10-14T16:47:15.290Z | † | 19.2.8 | An RSC transport fix only; the app uses no RSC. |
| react-dom | P31 | same | **19.3.1** | 2026-10-14T16:49:10.081Z | † | 19.2.8 | Peers `react ^19.3.1`. |
| @types/react | P31 | 19.3.0 | 19.3.0 | 2026-09-09T18:08:49.750Z | = | 19.2.18 | |
| @types/react-dom | P31 | 19.3.0 | 19.3.0 | 2026-09-09T18:07:51.886Z | = | 19.2.5 | |
| @tanstack/react-query | P31 | 5.104.1; 5.104.2 at D1 | **5.104.2** | 2026-10-11T19:35:15.506Z | † | 5.102.8 | 5.104.3 (2026-10-15T07:53:35Z) too young; query-core pinned with it. |
| dompurify | P31 | 3.4.16; 3.4.17 at D1 | **3.4.17** | 2026-10-12T12:41:28.880Z | † | 3.4.14 | IN_PLACE fixes. |
| marked | P31 | 18.1.0; 18.1.1 at D1 | **18.1.1** | 2026-10-12T12:29:51.405Z | † | 18.0.11 | Parser edge cases. |
| tailwind-merge | P31 | 3.7.0 | 3.7.0 | 2026-09-12T20:10:15.257Z | = | 3.6.0 | |
| @testing-library/dom | P31 | 10.4.2 | 10.4.2 | 2026-09-13T18:08:36.116Z | = | 10.4.1 | |
| @testing-library/user-event | P31 | 14.6.7 | **14.6.8** | 2026-10-13T06:41:22.306Z | **▲** | 14.6.6 | Treats a null input value as empty. Named nowhere in the plan or the driver. |
| @types/node | P31 | 26.6.5 | 26.6.5 | 2026-10-09T18:46:47.657Z | = | 26.4.0 | |
| jsdom | P31 | 30.1.2 | 30.1.2 | 2026-10-04T09:15:00.333Z | = | 30.0.1 | 30.1.3 (2026-10-15T09:24:38Z) too young. |
| prettier | P31 | 3.9.10 | 3.9.10 | 2026-10-10T08:35:28.472Z | = | 3.9.6 | Nothing to reformat. |
| vite | P32 | 8.3.4 | 8.3.4 | 2026-10-08T12:07:04.441Z | = | 7.3.6 | Brings rolldown 1.2.14 and lightningcss 1.33.0. |
| vitest | P32 | 5.0.3; 5.0.4 at D1 | **5.0.4** | 2026-10-12T09:47:02.826Z | † | 4.1.11 | 5.0.5 (2026-10-15T14:29:01Z) too young. `fsModuleCache` is now on by default. |
| @vitejs/plugin-react | P33 | 6.1.2 | 6.1.2 | 2026-10-05T10:08:27.143Z | = | plugin-react-swc 4.3.3 | Peers `vite ^8.0.0`. |
| @tauri-apps/api, plugin-dialog, cli | P31a | §2.2 | §2.2 | §2.2 | † | §2.2 | |

**Unchanged, each already the newest eligible:** @fontsource/ibm-plex-mono 5.3.0, @fontsource/ibm-plex-sans 5.3.0, clsx 2.1.1, tailwind-variants 3.3.1, zustand 5.0.15, @tailwindcss/vite 4.3.3, @testing-library/jest-dom 7.0.1, @testing-library/react 16.3.3, prettier-plugin-tailwindcss 0.8.1, tailwindcss 4.3.3, typescript 7.0.2 (`js/logs/00-targets.txt`). The one newer major not targeted is @babel/core 8.0.7, held on purpose by DU-W1 (Part III).

**Part III additions, registry facts only (not installed):** oxlint 1.88.0 (2026-10-13T09:47:46Z) and oxlint-tsgolint 7.0.2004, every platform binding eligible; knip 6.41.0; babel-plugin-react-compiler 1.0.0, @babel/core 7.29.7, @rolldown/plugin-babel 0.2.4 (peers `@babel/core ^7.29.0||^8`, `rolldown ^1.0.0-rc.5`, `vite ^8`, all met), @types/babel__core 7.20.5 (`registry/logs/11-cut-targets-npm.log`, `registry/logs/12-pins-npm-cutday.log`).

---

## 3. The proofs

### 3.1 Rust (the Rust leg unless said)

| Gate | Result | Log |
|---|---|---|
| P03: `.cargo/config.toml` deleted, then `cargo +1.99.0 metadata --locked` | PASS: exit 0, the lock byte-identical | `$S/rust/logs/01-p03-metadata-locked.log` |
| P31a manifest edit, then `cargo +1.99.0 update --dry-run --verbose` | exit 0, "Locking 38 packages"; held back by requirement: brotli-decompressor, generic-array, matchit, opener (0.9 major), serde-saphyr (2.0 major), the toml 0.8 trio | `$S/rust/logs/02a-update-dry-run.log` |
| P04: `cargo +1.99.0 update` | exit 0: 37 moved, 1 added (`static_vcruntime 3.0.0`) | `$S/rust/logs/02b-update.log` |
| Freshness gate, round 0 (`lockdates.py --gate --max-date 2026-10-14T19:53:00Z`, prep lock → updated) | exit 1 as expected: 38 added or moved, 25 eligible, 13 too young; every Tauri crate eligible | `$S/rust/logs/03-gate-00.log` |
| Step-back loop (apply the first `--precise` line, re-gate, repeat) | PASS after 11 steps: 26 added or moved, all eligible | `$S/rust/logs/03-gate-11.log`, `03-stepback-loop.summary`, each step's `03-step-NN.log` / `03-gate-NN.log` |
| Every entry of the final lock dated against an empty parent | PASS: 754 eligible, 0 too young (tries 1–2 ended with no verdict on crates.io HTTP 429) | `$S/rust/logs/06c-gate-full-lock-vs-dummy-try3.log` |
| The same, independently, with a fresh cache | PASS: 754 / 754, exit 0; the lock checked is `rust/locks/final.lock` (sha256 `19e96fd2…b374`) | `$S/verify/logs/01-gate-final-lock-vs-dummy.log` |
| Final lock: `metadata --locked`; pre-release and source scan | PASS: 765 packages, 0 pre-release (build metadata only, e.g. `+spec-1.1.0`), every entry from crates.io, no Tauri 3 alpha | `$S/rust/logs/04a-metadata-locked-final.log`, `04b-prerelease-and-sources.log` |
| Yanks | PASS: 0, from cargo's freshly revalidated index cache and from crates.io's API flags; the verifier's fresh read: 0 of 754 | `$S/rust/logs/16-yanked-from-index.log`, `17-newest-eligible.log`; `$S/verify/logs/05-yanked-final-lock.log` |
| rust-version of every resolved crate ≤ 1.99 | PASS: 765 resolved, 574 declare one, highest third-party takecell 1.96, sysinfo 1.95, tray-icon and tauri-utils 1.90 (registry leg over 756 pairs: same, 190 declare none) | `$S/rust/logs/08-msrv-check.log`; `$S/registry/logs/33-msrv-yank-cutday.log` |
| Duplicate majors | Every one held by a named third party; none by our crates (list below) | `$S/rust/logs/09e-dup-major-holders.log` (also 09a, 09b, 09c, 09f, 09g) |
| `cargo +1.99.0 check --locked --workspace --exclude openalpaca_gui --all-targets` | PASS: 0 warnings; one known future-incompatibility note (proc-macro-error2 2.0.1 ← aquamarine 0.6.0 ← teloxide 0.17.0) | `$S/rust/logs/07-check-workspace.log` |
| `cargo +1.99.0 clippy --locked --workspace --exclude openalpaca_gui --all-targets` | PASS: 0 triples, 0 `deprecated` | `$S/rust/logs/10-clippy-workspace.log`, `10b-clippy-workspace-count.txt` |
| `cargo +1.99.0 test --locked --workspace --exclude openalpaca_gui --no-fail-fast` | PASS: **5,126 / 0 / 17** in 26 harnesses (18 test binaries, 8 doc-test harnesses), each with its `ok` line; no `sleep 3600` or `sleep 30` orphan before or after | `$S/rust/logs/13-test-workspace.log` |
| The same figure on the prep tip | 5,126 / 0 / 17, recorded on 2026-10-12 on the combined prep tip (P36 + M08), a lock newer than the cut admits; no log in this run re-measures it | `tasks/deps-upgrade/README.md:25`, `tasks/todo.md:550`, plan-rev3 rev 3.12's log row |
| Live-eval: `clippy --locked -p openalpaca_core --features live-eval --all-targets` | PASS: 0 triples, 0 `deprecated` | `$S/rust/logs/15-clippy-live-eval.log` |
| G-GUI: `check -p openalpaca_gui --locked` (scratch target, empty `dist/`, placeholder sidecar) | PASS: 0 warnings, the whole 2026-10-14 Tauri family compiled | `$S/rust/logs/11a-gui-check.log` |
| G-GUI: `clippy -p openalpaca_gui --all-targets --locked`; `fmt -p openalpaca_gui --check` | PASS: 0 triples; 0 hunks | `$S/rust/logs/11b-gui-clippy.log`, `11c-gui-clippy-count.txt`, `11d-gui-fmt-check.log` |
| Lock difference | prep → final: 25 moved, 1 added, 0 removed (764 → 765); HEAD → final: 192 / 69 / 31 | `$S/rust/logs/05-lockdiff-prep-final.txt`, `05b-lockdiff-head-final.txt` |
| Workspace rustfmt drift | Pre-existing, not the toolchain's: 1,583 hunks in 304 files on the unmodified prep tip under both 1.93.0 and 1.99.0; unchanged by any change here (no hunk added). CI runs no fmt gate; M64 clears it | `$S/verify/logs/03-fmt-prep-1.93.0.log`, `03-fmt-prep-1.99.0.log`; `$S/rust/logs/26a-fmt-check-workspace-main.log` |

**Duplicate majors and their holders** (`rust/logs/09e`): the RustCrypto 0.10 and 0.11 halves (lopdf, tauri-codegen and pre-0.11 users); base64 0.22 (axum, reqwest 0.12) and 0.21 (swift-rs); reqwest 0.12 (teloxide-core); tower-http 0.6 (reqwest); tokio-tungstenite / tungstenite 0.29 (axum); zip 8.6 (calamine); darling 0.20 (derive_builder); hashbrown 0.14 and 0.16 (dashmap, twilight); syn 2 beside 3 (proc-macros); getrandom 0.3 (tauri, tempfile); png 0.17 (ico); indexmap 1 (schemars 0.8). The `--target all` list (`09c`) — the windows-sys / windows-targets families and gtk-rs 0.18 — was not given a holder per entry.

### 3.2 The step-backs (cargo `--precise`, dependents first)

Each release below was published after 2026-10-14T19:53:00Z, so the gate refused it and stepped back to the prep lock's version (or, for zip, to the newest eligible). None is a Tauri crate.

| Step | Command | Refused release (published) | Note |
|---|---|---|---|
| 1 | `cargo +1.99.0 update -p combine@4.6.9 --precise 4.6.8` | 2026-10-15T13:17:58Z | jni via tao and rustls-platform-verifier |
| 2 | `-p defmt@1.2.0 --precise 1.1.1` | 2026-10-15T14:45:39Z | Also moved defmt-macros 1.2.0 → 1.1.1 and defmt-parser 1.1.0 → 1.0.0; defmt is jiff's optional dependency and built on no target |
| 3 | `-p inout@0.2.4 --precise 0.2.2` | 2026-10-16T21:21:12Z | aead 0.6 / cipher 0.5 |
| 4 | `-p schannel@0.1.30 --precise 0.1.29` | 2026-10-16T16:28:12Z | Windows only |
| 5 | `-p unicode-normalization@0.1.26 --precise 0.1.25` | 2026-10-16T23:28:29Z | A direct dependency; data-path watch-list |
| 6 | `-p zbus_names@4.3.5 --precise 4.3.4` | 2026-10-16T14:49:32Z | Linux Secret Service |
| 7 | `-p zip@9.0.4 --precise 9.0.3` | 2026-10-16T09:09:57Z | 9.0.3 is newer than prep's 9.0.2 |
| 8 | `-p zstd-sys@2.1.2+zstd.1.5.7 --precise 2.1.1+zstd.1.5.7` | 2026-10-16T16:59:08Z | |
| 9 | `-p libc@0.2.191 --precise 0.2.190` | 2026-10-16T22:56:46Z | A direct dependency |
| 10 | `-p zvariant@5.15.1 --precise 5.15.0` | 2026-10-16T14:49:21Z | |
| 11 | `-p zvariant_derive@5.15.1 --precise 5.15.0` | 2026-10-16T14:49:09Z | After it the gate exits 0 |

The registry leg derived the same 13 releases from the index alone, before any resolve (`$S/registry/logs/29-derived-stepbacks.log`). Releases published between now and the cut will lengthen the list a cut-day `cargo update` prints; each is younger than the max date, so the gate steps it back and the run is expected to end on this same lock (not shown — §7).

### 3.3 JS (the JS leg unless said)

| Gate | Result | Log |
|---|---|---|
| Newest eligible per package at the max date | The only newer major not targeted: @babel/core 8.0.7 (DU-W1) | `$S/js/logs/00-targets.txt`, `00-cli-binaries.txt`, `00-peers-raw.txt` |
| P31 install from scratch (`rm bun.lock`, `bun install`, scratch `minimumReleaseAge` = 208,862 s, 2 s drift) | PASS: exit 0, no warnings; `lockfileVersion 2`, 219 entries / 204 names, 138 packages | `$S/js/logs/10-bun-install.log`, `10-install-age.txt`, `90-bun.lock`, `90-bunfig-scratch.toml` |
| Age of every `bun.lock` entry, transitive and platform packages included | PASS: 0 too young; the newest is @tauri-apps/cli-linux-riscv64-gnu 2.12.3 | `$S/js/logs/11-lock-ages.log` |
| The same, independently (`lockdates.py --pins npm` over the 218 pairs, fresh cache) | PASS: 0 too young, 0 pre-release | `$S/verify/logs/02-pins-npm-every-bun-lock-entry.log` |
| Releases the cut refused | Every refusal explained by date (§3.4) | `$S/js/logs/12-stepback-candidates.txt` |
| Peers on the installed tree (each resolved from its package's directory, checked with `Bun.semver`) | PASS: 31 edges, 0 unmet, 0 required peer missing | `$S/js/logs/13-peer-matrix-installed.log` |
| Peers from the registry (React 19.3, Vite 8, Vitest 5, jsdom 30, Part III) | PASS: 0 unmet; every absent peer optional | `$S/js/logs/14-peer-matrix-registry.log` |
| `engines.node` at 24.15.0 and 22.23.3 | PASS: 81 ranges, none fails | `$S/js/logs/15-engines-node.log` |
| `bun run check` (TypeScript 7.0.2) | PASS: 607 files | `$S/js/logs/20-bun-check.log` |
| `bun run test` (Vitest 5.0.4, jsdom 30.1.2) | PASS: **121 files / 1,551 tests** in 8.74 s; the 36 stderr blocks and 73 `act()` warnings are identical to HEAD's set | `$S/js/logs/21-bun-test.log` |
| `bun run format:check` (prettier 3.9.10) | PASS | `$S/js/logs/22-bun-format-check.log` |
| `bun run build` (`tsc -b`, Vite 8.3.4) | PASS: 306 modules; `dist/` 35 files / 1,059,491 B; 18 JS chunks / 777,639 B; CSS `index-BzoiVLez.css` 49,845 B (P32's anchor exactly); main chunk `index-lfDATJe7.js` 271,023 B; 3 `modulepreload` links | `$S/js/logs/23-bun-build.log`, `24-dist-size.txt`, `24-dist-sha256.txt` |
| `bun audit` | PASS: 0 in 203 packages | `$S/js/logs/25-bun-audit.log` |
| `bun outdated` | Three rows (react-query, jsdom, vitest), each marked held by the minimum release age | `$S/js/logs/26-bun-outdated.log` |
| P33 dev transform (Vite middleware mode, no port) | PASS: `$RefreshReg$` and the `/@react-refresh` preamble present with plugin-react 6.1.2 | `$S/js/logs/27-vite-dev-transform.log` |
| Baseline: HEAD's JS set (frozen v1 lock) | 121 / 1,551 in 10.70 s; 36 files / 1,020,842 B; 19 JS / 740,645 B; CSS `index-Bz42yHVK.css` 48,427 B — the plan's anchors | `$S/js/logs/50-baseline-head-frozen-install.log`, `51-…`, `52-…`, `52b-baseline-dist-size.txt` |
| CI shape: committed bunfig (604,800 s), `bun install --frozen-lockfile` from `apps/openalpaca-gui`, the four gates | PASS: all exit 0; the v2 lock accepted and left byte-identical; 121 / 1,551; `dist/` byte-identical (sha256) to the first build | `$S/js/logs/60-…`, `61-ci-shape-frozen-install.log`, `62-ci-shape-{check,test,format-check,build}.log` |
| Bun release dates | 1.4.3 the newest 1.4.x, 2026-10-10 | `$S/js/logs/44-bun-release-dates.txt` |

Against HEAD's JS set: `dist/` +38,649 B, JS +36,994 B (19 → 18 chunks), CSS +1,418 B (Lightning CSS); test wall time 10.70 → 8.74 s. The lock went from 267 names (v1) to 204 (v2): esbuild, rollup, @swc/core with its 12 binaries and @vitest/expect among the 81 that left; rolldown with its bindings, @oxc-project/types and @vitejs/plugin-react among the 18 that came. Nothing outside the Part I set was added.

### 3.4 The JS releases the cut refused

At the emulated instant `bun install` took the newest release at or before the max date: @tanstack/react-query 5.104.3 (2026-10-15T07:53:35Z) refused, 5.104.2 taken, and @tanstack/query-core 5.104.3 with it; jsdom 30.1.3 (2026-10-15T09:24:38Z) refused, 30.1.2 taken; vitest 5.0.5 (2026-10-15T14:29:01Z) refused, 5.0.4 taken, with @vitest/mocker and @vitest/spy 5.0.5. Transitively, by range: @asamuzakjp/css-color 7.1.6 (2026-10-16T13:13Z) refused although jsdom's `^7.1.3` admits it (7.1.5 taken), and undici 8.12.2 (2026-10-15T15:05Z) refused although `^8.11.2` admits it (8.12.1 taken) — which also shows the age filter worked. undici-types 8.12.2 is not a refusal: @types/node's `~8.9.0` never admitted it.

### 3.5 The Tauri pair (both legs)

| Gate | Result | Log |
|---|---|---|
| Tauri crate dates (`lockdates.py --pins crates`) | PASS: 10 eligible, each the newest release | `$S/js/logs/30-tauri-crate-dates.log`, `30b-tauri-crate-newest.txt` |
| JS pin dates, CLI and TypeScript platform binaries included | PASS: 61 checked, 0 too young (the Rust leg's 14-pin run: same) | `$S/js/logs/31-js-pin-dates.log`; `$S/rust/logs/12-pins-npm-tauri.log` |
| P31a's Rust half from the **prep tip's** lock | PASS in three steps: the planned command moved only dialog 2.8.3 and fs 2.7.1; `-p tauri --precise 2.12.4` moved seven crates; `tauri-plugin` needed its own update | `$S/js/logs/32-cargo-update-tauri.log`, `33-…-precise.log`, `34-…-tauri-plugin.log` |
| P31a's command as written from **HEAD's** lock (an export with P31a's manifest edits) | PASS in one command: tauri 2.12.4, the internals, plugin 2.7.2, dialog 2.8.3, fs 2.7.1, wry 0.57.0, tao 0.37.1; freshness gate 40 entries, all eligible | `$S/js/logs/41-head-p31a-cargo-update.log`, `43-lock-gate-head-vs-p31a.log` |
| Freshness gate, prep lock → pair lock | PASS: 11 entries, all eligible | `$S/js/logs/35-lock-gate-prep-vs-pair.log` |
| G-GUI check on the 2.12.4 pair | PASS: 0 warnings, the lock unchanged | `$S/js/logs/36-ggui-check.log` |
| The JS output reaching Rust: the same check with `--features tauri/custom-protocol`, so tauri-codegen 2.7.3 embeds the Vite 8 `dist/` | PASS: 0 warnings; the negative control (no `dist/`) fails, exit 101, the `frontendDist` panic | `$S/js/logs/37-ggui-check-embed-dist.log`, `38-ggui-embed-negative-control.log` |
| `bun x tauri info` | Reads tauri 2.12.4, tauri-build 2.7.3, dialog 2.8.3, cli 2.12.3; the JS api and plugin columns say "not installed!" (the known npm fallback under Bun) | `$S/js/logs/39-tauri-info.log` |
| The pair check across Cargo.lock, bun.lock and package.json | PASS (§2.2) | `$S/js/logs/40-pair-check.log` |
| The 15 Tauri-family entries in both legs' locks | Identical (verifier) | `$S/rust/locks/final.lock`, `$S/js/cargo-lock-pair.lock` |
| Tauri's changes, 10-10 → 10-14 (sparse index) | rust-version 1.90 throughout; tilde floors move together; tauri-build adds `static_vcruntime ^3.0` and `strsim ^0.11`; tauri-utils raises `swift-rs` to `^1.0.9`; plugin-dialog requires plugin-fs `^2.7.1`; nothing else | `$S/registry/logs/05-depdiff-tauri.log` |

### 3.6 Advisories

| Check | Result | Log |
|---|---|---|
| RustSec matcher calibration on HEAD's lock (advisory-db `64f92cd`) | 14 vulnerabilities — lopdf, rmcp, quick-xml ×2, rustls -0285/-0346/-0348/-0349, quinn-proto ×3, zip 2.4.2 ×3 — matching the review's B-09 figure | `$S/registry/logs/17-rustsec-calibrate-head-64f92cd.log`; `$S/rust/logs/14-advisories-head-ff0e7740.log` |
| RustSec on the prep lock | 7: rustls 0.23.45 -0346…-0349; zip 8.6.0 -0367/-0368/-0369 | `$S/registry/logs/19-rustsec-prep.log`; `$S/rust/logs/14-advisories-prep.log` |
| RustSec on the cut-day lock (own matchers; cargo-audit and cargo-deny are not installed) | **3**, all zip 8.6.0 via calamine (RUSTSEC-2026-0367/-0368/-0369, extract paths). Informational: proc-macro-error, proc-macro-error2, ttf-parser (unmaintained); glib 0.18.5 (unsound, Linux, via Tauri's gtk 0.18 — already on HEAD). With serde-saphyr 2.0: identical | `$S/rust/logs/14-advisories-final.log`; `$S/registry/logs/19-rustsec-cutday.log`, `19-rustsec-cutday-saphyr2.log` |
| Every dependent of rustls / tokio-rustls / hyper-rustls / aws-lc-rs | Every requirement is `^0.23.x` (minimum 0.23.27) and admits 0.23.46; tauri's optional rustls + ring applies to Android and iOS only | `$S/registry/logs/07-rustls-dependents.log` |
| Repository advisories (tauri, plugins-workspace, wry, rustls, zip2) | tauri 2.12.4 clear of GHSA-w28w-mhc8-qvjv (≤ 2.11.5) and GHSA-7gmj-67g7-phm9 (≤ 2.11.0); plugin-fs 2.7.1 clear of GHSA-9g54-6x48-9vpw (< 2.5.2); rustls 0.23.46 clear of the four 10-14 advisories; zip 9.0.3 clear | `$S/registry/logs/25-ghsa-repo-advisories.log` |
| GitHub's global advisory database, Rust | No verdict: its calibration returned 0 where real advisories exist (zip 8.6.0, rustls 0.23.45, tauri 2.11.5); RustSec and the repository advisories are the sources used | `$S/registry/logs/22-ghsa-rust-cutday.log`, `23-ghsa-rust-zip8-rustls45.log` |
| `bun audit` (npm's bulk advisory endpoint) | 0 in 203 packages | `$S/js/logs/25-bun-audit.log` |
| GitHub's global advisory database, the 39 JS direct pins | 0 (calibrated: HEAD's versions return the two dompurify advisories). **Out of process:** `api.github.com/advisories` was not an allowed endpoint for these legs, and the JS leg declined it for that reason; the result is useful and covers direct pins only | `$S/registry/logs/20-ghsa-npm-cutday.log`, `21-ghsa-npm-calibrate-head.log` |

The dompurify count is unsettled: the review named three low advisories against ≤ 3.4.16; GitHub shows two, both patched in 3.4.16. 3.4.17 is clear either way.

---

## 4. Incompatibilities and fixes

Ordered by what changes code or the plan's commands first.

**C1 — The prep tip's P30 refuses a skill file with a long comment run, where HEAD reads it.** *(Independent of D4.)*
- *Evidence.* `yaml_options()` in the prep tip (`crates/openalpaca_core/src/middleware/skill/mod.rs:54-63`) sets no `emit_comments`. Under serde-saphyr's default comment handling — 1.3 always, 2.0 with its default-on `comments` feature — a frontmatter with 33 or more consecutive comment lines between a mapping key and its block value is refused, `too many consecutive comments before resolving collection entry`; 32 pass, 33 fail; comment runs elsewhere pass. serde_yaml 0.9 (HEAD) reads all of them (`$S/saphyr/logs/30-probes3-comments-on.log`, `32-probes-all-emit-on.log` / `-off.log`). With `emit_comments: false`, on either version, the four refused probes read equal to serde_yaml and nothing else changes; E6's `yamlcmp` on 2.0 goes from 4 typed parse errors to 0 (`41-yamlcmp-e6-2.0-final-probes.log`, `42-yamlcmp-e6-1.3-probes-all.log`). The pin test fails without the option (`36-mutation-without-emit-comments-pin-fails.log`, exit 101) and the core suite passes with it, 2,546 / 7 (`37-final-2.0-test-core-full.log`). None of the four repository skills hits it (`13-…`, `40-yamlcmp-e6-2.0-final-real.log`). The Rust leg's suite is green only because no test covers it; its variant patch carries only the 2.0 rename (`$S/rust/logs/91-variant-opener-saphyr.patch`).
- *Fix.* Add `emit_comments: false,` to `yaml_options()`, with a doc-comment line saying a run of comments is read the way serde_yaml read it, and a pin test (the scratch `comment_runs_and_non_finite_floats_saphyr_leg_pin` in `$S/saphyr/patch-final-recommended.diff`, rustfmt-formatted: as drafted its `.map(...).collect()` chain adds one hunk, `44-rustfmt-check-final-skill-module.log`). Equivalent, measured equal on all 42 probes but 2.0-only: `serde-saphyr = { version = "2.0", default-features = false, features = ["serialize", "deserialize"] }`. The option is the better form: explicit in code, immune to Cargo feature unification, and valid on 1.3 too. It restores P30's own stated aim ("as close to serde_yaml's reading as its options go"), so it is a fix inside P30's accepted design — no new enforcement point, no reversed row — and this report recommends it for P30 under either D4 answer; the saphyr leg presented it and did not adopt it, and P30's author writes it in.
- *Plan change.* P30's contents gain the option and the pin test: `middleware::skill` 20 → 21 tests, the core lib 2,545 → 2,546 (the workspace figure +1 expected, not measured). E6's `yamlcmp` takes the same option. P38's Gotchas line on the saphyr options names it. Whether it matters in practice is decided by E6 against the owner's real skill directories (§7).

**C2 — The prep tip's Tauri pair is stale, and P31a's command from a 2.12.3-family lock leaves a mixed family.**
- *Evidence.* The prep tip pins `tauri-plugin-dialog = "=2.8.2"` (`refs/prep/deps-wave-2026-10-12:Cargo.toml:178`) and locks tauri 2.12.3, tauri-build 2.7.2, tauri-utils 2.10.1, runtime(-wry) 2.12.1, codegen and macros 2.7.2, plugin 2.7.1, plugin-fs 2.7.0; its JS half is still HEAD's 2.11.1 / 2.7.3 / 2.11.4. Against the cut's npm 2.12.4 / 2.8.3 the dialog plugin's exact pair fails (2.8.2 ≠ 2.8.3). From that lock, P31a's `cargo update -p tauri -p tauri-build -p tauri-plugin-dialog` with `=2.8.3` moves only dialog 2.8.3 and fs 2.7.1 and leaves tauri 2.12.3 over the 10-10 internals — a combination never released together (`$S/js/logs/32-cargo-update-tauri.log`); `-p tauri --precise 2.12.4` moves seven crates but leaves `tauri-plugin` at 2.7.1 until its own update (`33-…`, `34-…`). From HEAD's lock the same planned command takes the whole 10-14 family at once (`41-…`, `43-…`). The Rust leg's whole-lock `cargo update` moved everything regardless and so hid it.
- *Fix.* P31a's Rust half at D1's values: `tauri-plugin-dialog = "=2.8.3"` and the 2.12.4 family. From HEAD's lock (the branch's real base) the planned command as written suffices. From the prep tip, or any lock already on the 2.12.3 family, add `cargo update -p tauri --precise 2.12.4` and `cargo update -p tauri-plugin` (or name every internal with `-p`).
- *Plan change.* P31a's body names both cases. A G-base or rehearsal run from the prep tip uses the driver's `TAURI_PRECISE` path with `--tauri-crate` overrides (tauri-plugin 2.7.2 among them); step 1b's exact check refuses a mixed family. The JS side needs nothing beyond the planned pins.

**C3 — The prep tip's other floors are below MF-2's rule, and opener 0.9 is missing.**
- *Evidence.* The prep tip writes `rustls = { version = "0.23", … }` (`Cargo.toml:148`), `sha2 = "0.11"` (`:156`), `zip = { version = "9.0.2", … }` (`:164`), `keyring-core = "1.0"` (`:170`) and `opener = "0.8"` (`:175`). The cut-day lock resolves rustls 0.23.46, sha2 0.11.1, keyring-core 1.0.1 and zip 9.0.3 under those requirements anyway, but MF-2 writes each floor equal to the version taken — and `"0.23"` admits every rustls release that RUSTSEC-2026-0349 affects (it has no unaffected range below 0.23.46). opener: P27a (DU-24 (a), taken) plans 0.9.0, eligible since 2026-10-06; the re-cut commit `6992a402` never touched it, so the "every planned Rust move as of 2026-10-12" premise is wrong for this one item. In a scratch variant, `opener = "0.9"` moves only opener in the lock (`$S/rust/logs/20a-variant-opener-update.log`; *rev 2:* only opener's version — the same update re-points 21 dependency edges to versions already in the lock, §8.3), clippy is 0 (`20c-…`), and the suite is green with serde-saphyr 2.0 stacked on top (`22-variant-test-workspace.log`). Our one call, `opener::open` at `apps/openalpacad/src/routes/files_types.rs:163` (prep tip), compiles unchanged.
- *Fix.* The floors of §6.1; land P27a as written (`opener = "0.9"`, `cargo update -p opener`, no source change).
- *Plan change.* P41 (prep `96a1dc30`), P10 (`b899046c`), P12 (`44ec0b03`) and P16 (`5c265232`) are re-pinned when they land (§5.1 item 10). Any "measured cut-day set" adds opener 0.9.0 to the prep tip. P27a's gates (G-CI; the Linux Open check, deferred under DU-33) apply as written.

**C4 — Six JS literals in the plan's measured-set text are below the newest eligible, and one is named nowhere.**
- *Evidence.* `$S/registry/logs/11-cut-targets-npm.log`, `12-pins-npm-cutday.log`: react and react-dom 19.3.1 against P31's 19.3.0; @tanstack/react-query 5.104.2 against 5.104.1; @testing-library/user-event **14.6.8 against 14.6.7** (and against the driver's `JS_PINS`, `gbase.sh:433`); dompurify 3.4.17 against 3.4.16; marked 18.1.1 against 18.1.0; vitest 5.0.4 against P32's 5.0.3. All patch releases; their notes show bug fixes only. Rev 3.13 already names all but user-event at D1's instant.
- *Fix and plan change.* The literals of §6.2 in P31, P32 and the driver; DU-23's minute table re-derived; the §1.3 rows stay dated records. Releases newer than these (react-query 5.104.3, vitest 5.0.5, jsdom 30.1.3) are left to §5.1 item 5's before-the-merge sweep.

**C5 — `bunfig.toml` counts seven days from the install; DU-23 counts from the cut instant.**
- *Evidence.* The committed `minimumReleaseAge = 604800` admits "the newest version published at least seven days ago" at the moment of install. The direct pins are exact, but every transitive package floats in its range, so an install after the cut admits transitive releases published after the max date — undici 8.12.2 and @asamuzakjp/css-color 7.1.6 today (§3.4). The JS leg's lock is valid only because its scratch bunfig put the limit exactly at the max date (`$S/js/logs/90-bunfig-scratch.toml`). `lockdates.py` has no `bun.lock` reader (its usage, `lockdates.py:8`, takes `--pins crates|npm` and NAME@VERSION arguments); the verifier extracted the 218 pairs by hand to run it.
- *Fix.* After each JS lock write (P31, P32, P33, P31a's JS half, M66, M67, M69), run `lockdates.py --pins npm --max-date 2026-10-14T19:53:00Z` over every name@version of the new `bun.lock` — a small reader in `lockdates.py`, or an extraction step before it. Alternatively, run each install with `minimumReleaseAge` set to (install time − 2026-10-14T19:53:00Z) in seconds, as the JS leg did.
- *Plan change.* §5.2's freshness gate and the gates of P31, P32, P33, P31a and M66/M67/M69. Without it the JS locks can exceed DU-23 unnoticed. (Whether bunfig's install-time rule should govern transitive JS packages instead is a reading of DU-23 for the owner, not a default to adopt.)

**C6 — zip 8.6.0, kept by calamine 0.36.1, carries three RustSec advisories, and no eligible release fixes them.**
- *Evidence.* RUSTSEC-2026-0367, -0368 and -0369 match zip 8.6.0, each patched in ≥ 9.0.0 (`$S/registry/logs/19-rustsec-cutday.log`). calamine 0.36.1 (2026-07-27) is the newest release and requires `zip ^8.6`; the 8.x line has nothing after 8.6.0 (`04-cut-targets-crates.log`). All three concern `extract()` / `ZipStreamReader`, which neither calamine nor our code calls.
- *Fix.* No version fix exists at the cut. Keep P34's three active ignores with the reachability reason. The way out is a calamine release requiring zip `^9` (an upstream PR, the owner's public action) or a zip 8.x backport; neither exists today.
- *Plan change.* P34 and E2 expect 3 ignored vulnerabilities, not 0; cargo-audit's configuration carries the same ignores. Re-check on the cut day.

**C7 — The two advisory databases disagree on zip 9.0.0.**
- *Evidence.* GHSA-4684-h2q9-2vcp (zip2's repository advisory, 2026-10-10) says vulnerable `<= 9.0.0`, patched 9.0.1; its RustSec import, RUSTSEC-2026-0369, says patched `>= 9.0.0` (`$S/registry/logs/25-ghsa-repo-advisories.log`, `19-rustsec-prep.log`).
- *Fix and plan change.* One sentence in P16's floor rule: our zip's floor and lock never below 9.0.1 (a cargo-deny run on a 9.0.0 lock would pass wrongly). The cut-day 9.0.3 satisfies both readings.

**C8 — P04's recorded step-backs are all moot at this cut; a different set is needed.**
- *Evidence.* uuid 1.28.0, serde_json 1.0.154, signal-hook 0.4.5, tokio-util 0.7.20, toml 1.1.8 and toml_edit 0.25.17 are all eligible (`$S/registry/logs/04-cut-targets-crates.log`); 13 other releases are not (§3.2).
- *Fix and plan change.* P04's body and gate: the step-backs are what the freshness gate prints on the day, applied dependents first and re-gated after each step (§3.2 is today's lower bound); `gbase.sh --apply-step-backs` runs the cascade.

**C9 — Two new transitive entries arrive with the cut-day versions.**
- *Evidence.* aws-lc-rs 1.18.2 requires aws-lc-sys `^0.46.0`, a new 0.x minor (prep has 0.45.0) bundling AWS-LC 5.11.0 (was 5.7.0); its notes say non-canonical Ed25519 public keys are now rejected, and the system-library minimum rose to 5.7.0 for non-FIPS system-library builds only (bundled builds, ours, unchanged) (`$S/registry/logs/06-depdiff-others.log`). tauri-build 2.7.3 adds static_vcruntime 3.0.0 (2025-11-25, rust-version 1.79, no dependencies, no advisory; Windows only) (`05-depdiff-tauri.log`). aws-lc-sys 0.46.0 compiled in the Rust leg's macOS build; static_vcruntime was resolved and never compiled.
- *Fix and plan change.* Name both in P04's and P31a's lock-change bodies; add aws-lc-sys to the data-path watch-list (§5.3 rule (2): new transitive majors and minors named).

**C10 — The prep tip's P30 doc comment misdescribes non-finite floats.** *(Independent of D4.)*
- *Evidence.* `yaml_options()`'s doc comment (prep `mod.rs:52`) says a `.inf` value "stays the text it was written as". In `serde_json::Value` positions (`tools.defaults`, `scripts[].parameters`, the buffered `context.sources` enum), 1.3 and 2.0 with `AsString` give canonical strings instead: `.NaN` → `".nan"`, `+.Inf` → `".inf"`, `1e999` → `".inf"`; serde_yaml 0.9 gave null (and `"1e999"`). String-typed fields keep the text as written under every variant (`$S/saphyr/logs/14-yamlcmp3-probes-comments-off.log`, `26-yamlcmp3-probes-passthrough.log`, `29-yamlcmp3-p23-typed-debug.log`).
- *Fix and plan change.* One sentence in P30's doc comment: Value positions get the canonical spelling. 2.0's `NonFiniteFloatPolicy::PassThrough` would restore serde_yaml's null for `.inf`/`.nan` (not for `1e999`); that is a design choice for P30's author, presented, not adopted. No effect on the four repository skills.

**Residue, not incompatibilities.** glib 0.18.5 is unsound (Linux, through Tauri's gtk 0.18; on HEAD already); proc-macro-error2 (through teloxide → aquamarine) is unmaintained and behind the one future-incompatibility note (RUSTSEC-2026-0173); ttf-parser and proc-macro-error are unmaintained. lightningcss is duplicated in the JS tree (1.32.0 pinned exactly by @tailwindcss/node 4.3.3, 1.33.0 from vite 8.3.4 — two native binaries until Tailwind moves), as are magic-string 0.30.21 / 1.4.3 and the older aria-query and dom-accessibility-api pairs. `pnpm-workspace.yaml`'s `onlyBuiltDependencies: esbuild` is dead once esbuild leaves the lock (P37 deletes it).

---

## 5. serde-saphyr 2.0 (D4)

D4's recommendation (`reports/2026-10-17-plan-review.md`, D4) was to research 2.0 first and then take the newest eligible 2.x if the research holds, or name the blocker and its way out — never pin 1.3 silently. The research is done. What it found:

- **Eligibility.** serde-saphyr 2.0.0 (2026-10-13T15:11:53Z, rust-version 1.89) is the only 2.x; granit-parser 2.0.0 (2026-10-13T09:17:05Z) comes with it. Both are eligible at the cut, no step-back is needed, and the lock moves those two packages only (`$S/saphyr/logs/05-update-serde-saphyr-2.0.log`, `06-lockdates-gate-1.3-to-2.0.log`). No RustSec advisory for either (`33-rustsec-advisory-db.log`). Only `openalpaca_core` depends on it (`01-tree-i-serde-saphyr-1.3.log`). Nothing newer can become eligible by the cut; only a yank of 2.0.0 could change the answer.
- **One forced change.** As a drop-in it compiles and passes 20 / 20 `middleware::skill` tests with one rustc `deprecated` warning, which breaks G-lint's 0-`deprecated` rule: `Options::reject_non_finite_typeless_float` is deprecated for `non_finite_float_policy` (`07-dropin-2.0-test-middleware-skill.log`; rustc reports it at `mod.rs:55`, inside the `options!` macro; the field is at `:59`). The replacement `non_finite_float_policy: serde_saphyr::NonFiniteFloatPolicy::AsString,` is the exact equivalent — serde-saphyr 2.0.0's own `src/de/cfg.rs:44-48` maps the default policy with `reject = false` to `AsString` — and the probes agree on every input. `#[allow(deprecated)]` is not an option under the no-compromise rule.
- **With that change.** Core suite 2,545 / 7 + 16 + 1 / 1, the 1.3 baseline's counts (`10-…`); daemon `routes::skills` 8 / 8 (`15-…`); core clippy 0 (`09-…`); in the Rust leg's variant the whole workspace 5,126 / 0 / 17, live-eval clippy 0, the freshness gate exit 0 and no fmt hunk added (`$S/rust/logs/21c-…`, `22-…`, `23-…`, `24-…`, `26a`/`26b`).
- **Behaviour.** On the four repository skills (`config/skills/{code-review,commit-message,create-skill,explain-code}`) every facet is identical — typed `SkillFrontmatter`, `serde_json::Value`, the renderer's bytes, render-then-reparse, serde_yaml against 2.0 — with comments on and off (`13-…`). On 42 synthetic edge cases 1.3 and 2.0 accept and reject the same inputs with equal values and render bytes; only error texts differ, and 2.0's are cleaner: 1.3's spurious alias-style location chains from the `LastWins` replay are gone, a genuine alias reads `(defined at …) (used at …)`, the multiple-documents hint names `from_str_multiple`. Every error stays one line; no test pins the text; the tree never matches `AliasError` or uses `from_multiple*`, `Commented<T>` or `strict_indentation` (`14-…`, `26-…`).
- **The comment feature.** 2.0's default-on `comments` feature is behaviour-neutral for us. The 32-comment limit (C1) exists in 1.3 too, so it is P30's gap, not 2.0's, and `emit_comments: false` closes it on either version.
- **E6.** `yamlcmp` pins `serde-saphyr = "=1.3.0"` (HEAD `tasks/deps-upgrade/tools/e6/yamlcmp/Cargo.toml:17`), names 1.3.0 in its module doc (`src/main.rs:2`), sets the deprecated field (`src/main.rs:40`), and the E6 README's row says "serde-saphyr 1.3.0" (`tasks/deps-upgrade/tools/e6/README.md:22`). Rebuilt in scratch against `=2.0.0` with the migrated option it builds with 0 warnings and gives the 1.3 build's verdicts on the real files and on all probes (`23-…`, `24b-…`, `24c-…`, `24d-…`); with `emit_comments: false` too, its typed parse-error count goes 4 → 0 (`41-…`).

**What this means for D4.** The research D4 asked for holds: no blocker was found, and 2.0 costs one line of source beyond what P30 needs anyway (C1, C10). The decision remains the owner's; this report does not take it.
- Under **D4 (a)**, P30 writes `serde-saphyr = "2.0"` (default features) with `non_finite_float_policy: AsString` in place of the deprecated field, `emit_comments: false`, the pin test, and the corrected doc comment (the full scratch diff is `$S/saphyr/patch-final-recommended.diff`); DU-4, §1.2's and §1.6's rows and P30's "newest is 1.3.0" are re-anchored to 2.0.0; E6's `yamlcmp` moves to `=2.0.0` with the same options (its `Cargo.toml:17` and comment, `main.rs:2-3` and `:40`, its `Cargo.lock`, `README.md:22`), and plan-rev3 §6 E6's "serde-saphyr 1.3" becomes 2.0.
- Under **D4 (b)**, P30 keeps 1.3.0 with `emit_comments: false`, the pin test and the doc fix, and under the no-compromise rule the hold is named in §3.1 with 2.0.0 as its way out.

---

## 6. What the plan's D1 re-anchoring must write

Plan-rev3's rev 3.13 text says no literal is re-anchored before D1 is answered, and then P31a, DU-23's table, the driver's pair and JS pins and step 1b's reference lock are. D1 is answered; these are the values. *Rev 2:* plan-rev3 rev 3.15 writes them into its entries, modernization-plan rev 2.4 into Part III's, and the driver's defaults carry §6.3's (the note under its table).

### 6.1 Manifests and Rust commands

`[workspace.dependencies]` in the root `Cargo.toml`:

| Commit | Requirement | Lock | Prep tip writes |
|---|---|---|---|
| P31a | `tauri = "~2.12"` | 2.12.4 | same |
| P31a | `tauri-build = "~2.7"` | 2.7.3 | same |
| P31a | `tauri-plugin-dialog = "=2.8.3"` | 2.8.3 | `"=2.8.2"` |
| P41 (and P04's gate) | `rustls = { version = "0.23.46", default-features = false, features = ["aws_lc_rs"] }` | 0.23.46 | `"0.23"` |
| P10 | `sha2 = "0.11.1"` | 0.11.1 | `"0.11"` |
| P12 | `keyring-core = "1.0.1"` | 1.0.1 | `"1.0"` |
| P16 | `zip = { version = "9.0.3", default-features = false, features = ["deflate"] }`, then `cargo update -p zip@<locked> --precise 9.0.3`; never below 9.0.1 (C7) | 9.0.3 | `"9.0.2"` |
| P27a | `opener = "0.9"` | 0.9.0 | `"0.8"` |
| P30 | `serde-saphyr = "1.3"` under D4 (b), `"2.0"` under D4 (a); `emit_comments: false` either way (recommended, C1: a fix inside P30's stated aim) | 1.3.0 / 2.0.0 | `"1.3"` |
| unchanged | `rmcp` 3.5.1, `process-wrap = { version = "10.0.1", features = ["tokio1"] }`, `toml = "1.1"` (1.1.8), `toml_edit = "0.25"` (0.25.17), `calamine = "0.36"` (0.36.1, keeping zip 8.6.0) | | same |

- **P03:** `rust-toolchain.toml` `channel = "1.99.0"`, `rust-version = "1.99"`, `.cargo/config.toml` deleted — unchanged by D1; P03b to 1.99.1 later (D3 (a)).
- **P31a's Rust half:** from HEAD's lock, `cargo update -p tauri -p tauri-build -p tauri-plugin-dialog` takes the whole family; from a 2.12.3-family lock add `cargo update -p tauri --precise 2.12.4` and `cargo update -p tauri-plugin` (C2). No Tauri step-back. The body names the family exactly — tauri 2.12.4, tauri-build / -codegen / -macros 2.7.3, tauri-runtime(-wry) 2.12.2, tauri-utils 2.10.2, tauri-plugin 2.7.2, tauri-plugin-fs 2.7.1, tauri-plugin-dialog 2.8.3, wry 0.57.0, tao 0.37.1, swift-rs 1.0.9 — and the added static_vcruntime 3.0.0 (C9).
- **P04:** the step-backs are what the freshness gate prints on the day (§3.2 is today's lower bound: 11 steps over 13 releases; the defmt step cascades to defmt-macros and defmt-parser); rev 3.1's six are moot (C8). The lock-change body names aws-lc-rs 1.18.2 / aws-lc-sys 0.46.0 (C9) and tokio 1.53.3. Its gate: RUSTSEC-2026-0285 and -0346…-0349 gone, rustls 0.23.46 taken, D2 moot.
- **P34 / E2:** three reasoned ignores for zip 8.6.0 (C6).

### 6.2 JS pins

| Commit | Pins (exact) |
|---|---|
| P42 | root `"packageManager": "bun@1.4.3"` (unchanged) |
| P31a | `@tauri-apps/api` 2.12.4, `@tauri-apps/plugin-dialog` 2.8.3, `@tauri-apps/cli` 2.12.3 |
| P31 | `react` **19.3.1**, `react-dom` **19.3.1**, `@types/react` 19.3.0, `@types/react-dom` 19.3.0, `@tanstack/react-query` **5.104.2**, `dompurify` **3.4.17**, `marked` **18.1.1**, `tailwind-merge` 3.7.0, `@testing-library/dom` 10.4.2, `@testing-library/user-event` **14.6.8**, `@types/node` 26.6.5, `jsdom` 30.1.2, `prettier` 3.9.10 |
| P32 | `vite` 8.3.4, `vitest` **5.0.4** |
| P33 | `@vitejs/plugin-react` 6.1.2 (replacing `@vitejs/plugin-react-swc` 4.3.3) |

Bold marks a value that differs from the plan's measured-set text. Each JS lock write is followed by C5's `bun.lock` freshness check. P31/P32/P33's build anchors are re-recorded from each commit's own run: this report's figures — 306 modules, 18 JS chunks / 777,639 B, CSS `index-BzoiVLez.css` 49,845 B, 3 `modulepreload` links — are of P31 + P32 + P33 + P31a's JS half together (P32's text has 775,895 B for the JS chunks; the CSS hash and size are P32's exactly).

### 6.3 The G-base driver (`tasks/deps-upgrade/tools/gbase/gbase.sh` at HEAD)

| Line | Today | At D1 (a) |
|---|---|---|
| `:42` (and the comment at `:40-41`) | `NOT_BEFORE="2026-10-17T20:08Z"` | `NOT_BEFORE="2026-10-21T19:53Z"` |
| `:50` | `TAURI_PRECISE="2.12.3"` | `"2.12.4"` |
| `:51` | `DIALOG="2.8.2"` | `"2.8.3"` |
| `:52` | `JS_API="2.12.3"` | `"2.12.4"` |
| `:53` | `JS_CLI="2.12.2"` | `"2.12.3"` |
| `:385` | `TAURI_REF="$HERE/locks/guiD-lock-01b-pair.lock"` (the 2.12.3 pair's internals) | re-derived for 2.12.4 — otherwise step 1b checks the internals against 2.12.3's and refuses the family. The 15 Tauri entries of `$S/rust/locks/final.lock` and `$S/js/cargo-lock-pair.lock` are identical and are a ready source (the driver README's "The cut instant" says how). Until then, pass all eight: `--tauri-crate tauri-build=2.7.3`, `tauri-codegen=2.7.3`, `tauri-macros=2.7.3`, `tauri-runtime=2.12.2`, `tauri-runtime-wry=2.12.2`, `tauri-utils=2.10.2`, `tauri-plugin=2.7.2`, `tauri-plugin-fs=2.7.1` |
| `:426` | `@tanstack/react-query … 5.104.1` | `5.104.2` |
| `:427` | `dompurify … 3.4.16` | `3.4.17` |
| `:428` | `marked … 18.1.0` | `18.1.1` |
| `:429` | `react … 19.3.0` | `19.3.1` |
| `:430` | `react-dom … 19.3.0` | `19.3.1` |
| `:433` | `@testing-library/user-event … 14.6.7` | `14.6.8` |
| `:441` | `vitest … 5.0.3` | `5.0.4` |

The other nine `JS_PINS` rows (`:431-432`, `:434-440`) already hold the cut-day values. Until the defaults are re-anchored, the same run is `--not-before 2026-10-21T19:53Z --tauri 2.12.4 --dialog 2.8.3 --js-api 2.12.4 --js-cli 2.12.3`, the eight `--tauri-crate` values above, and seven `--js-pin` overrides (react 19.3.1, react-dom 19.3.1, @tanstack/react-query 5.104.2, dompurify 3.4.17, marked 18.1.1, @testing-library/user-event 14.6.8, vitest 5.0.4). The suite figure measured on the cut-day lock is 5,126 passed; C1's pin test adds one (expected, not measured).

*Rev 2:* the defaults above are re-anchored in the working tree of this step — `NOT_BEFORE` 2026-10-21T19:53Z, `TAURI_PRECISE` 2.12.4, `DIALOG` 2.8.3, `JS_API` 2.12.4, `JS_CLI` 2.12.3, the seven `JS_PINS` values of the right-hand column, and step 1b's reference re-derived as `locks/d1-lock-01b-pair.lock`, which replaces `guiD-lock-01b-pair.lock` — so the "Today" column is HEAD `6d3e47fc`'s and the overrides are no longer needed. The driver's line numbers moved with it; its README's status line and its 2026-10-17 revision-log rows are the record of what landed.

### 6.4 Other text

- DU-23's minute table, re-derived at 2026-10-14T19:53:00Z (this report's §2 is that derivation).
- §1.2, §1.3 and §1.6 stay dated records (SF-19); each row the cut supersedes is marked so.
- §5.2's freshness gate gains C5's `bun.lock` check; P16's floor rule gains C7's sentence; P30 gains C1's option and pin test and C10's doc sentence under either D4 answer (recommended, C1).
- The plan's [NE] on the wry and tao dates is closed (§2.2).

---

## 7. Not established

Nothing below was measured; each is named so that no cap is silent.

**Whole-wave and runtime**
- **The combined tree's own gaps** (rev 2; rev 1 read "no single tree carried both halves"). §8 built, gated and debug-bundled the two halves in one tree, in scratch. On its exact lock the workspace suite, check and clippy did not run: rev 1's 5,126 / 0 / 17 and clippy 0 are of `final.lock`, without opener 0.9, and rev 1's opener variant ran the suite only with serde-saphyr 2.0 on top. The G-base run at the cut is the measurement of record.
- **Nothing at runtime:** no app launch, no G-bundle legs, `package-macos.sh` or E5; no IPC between `@tauri-apps/api` 2.12.4 and the 2.12.4 runtime; no WKWebView rendering of the Lightning CSS output. The Rust leg's G-GUI used an empty `dist/` and a placeholder sidecar; the JS leg's check embedded the real `dist/`, and so did §8's debug bundle (rev 2), which was built, checked by G-ident's debug variant and never opened. No release bundle or `libexec/` copy was made, so E5's release G-ident is unmeasured.
- **Cut-day reproducibility is expected, not shown.** A yank before 2026-10-21T19:53Z would change both locks; a later `cargo update` will print a longer step-back list; a later `bun install` under the committed 604,800 s rule will take newer transitive releases (C5). The yank scans must re-run on the cut day.
- **The "5,126 / 0 / 17, the same as prep" comparison** rests on the 2026-10-12 record (§3.1), measured on a newer-than-cut lock; no log in this run re-measures the unmodified prep tip's workspace suite (the saphyr leg measured core alone, 2,545 / 7, which matches core's share).

**Rust**
- **opener 0.9.0 alone** (P27a without serde-saphyr 2.0) never went through the full workspace suite; its lock move and clippy were measured alone, the suite only with 2.0 stacked. *Rev 2:* §8's tree is opener 0.9 without 2.0 — its lock move, the freshness gate, the daemon's build (0 warnings) and G-GUI passed there; the suite and workspace clippy still did not run with it. Neither variant was compiled with the shell (the shell's graph contains neither crate), and neither went through the step-back cascade beyond its own three entries (`$S/rust/logs/23-variant-gate-opener-saphyr.log`).
- **Linux and Windows were never compiled** (DU-33): static_vcruntime 3.0.0 (Windows), the gtk-rs 0.18 / glib 0.18.5 set, schannel, zbus and the windows-sys / windows-targets families are resolved, not built; the `--target all` duplicate list has no holder per entry.
- **aws-lc-sys 0.46.0** (AWS-LC 5.11.0, non-canonical Ed25519 keys rejected) compiled on macOS; no test aimed at TLS or key handling was named as covering it, and its build requirements on other hosts were not checked.
- **Whether ring is built on macOS.** The lock lists ring 0.17.14 under quinn-proto and rustls-webpki (`$S/registry/logs/38-ring-in-prep-lock.log`); `cargo tree -e features -i ring` decides and was not run here (P41's gate runs it after P28).
- **No cargo-audit or cargo-deny run** (neither is installed; installing was out of scope). The advisory results are two legs' own matchers against one advisory-db commit, `64f92cd`; the plan's pinned run (§5.1 item 13) is still owed. The Rust leg's yank sweep read cargo's local index cache, which file mtimes cannot prove fresh; the crates.io API flags and the verifier's fresh read are the independent confirmation.
- **The E6 owner checks were not run, by rule:** the real TOML files through toml_edit 0.25.17 (P04's gate), and the owner's real skill directories under `~/.openalpaca` (and any plugin-contributed `SKILL.md`). The latter decides whether C1 and C10 bite in practice: whether any real skill has a run of 33 or more comment lines, or a non-finite float in a Value position, is unknown.
- **serde-saphyr 2.0 beyond the probes:** full equivalence with 1.3 on every input (2.0's composite-mapping-key comparison was probed only in ignored fields and Value positions); parse speed and memory; the published crate's CHANGELOG heading "## 2.0.0 Unreleased" (its checksum matches crates.io; upstream's intent was not checked). Which non-finite policy P30 takes (`AsString` or `PassThrough`) and whether it takes `emit_comments: false` are P30's author's or the owner's choices, presented, not adopted.
- **Part III crates beyond one dependency level** (e.g. jsonschema's transitive strum_macros and the uuid-simd / vsimd crates).
- **Whether calamine has an unreleased move to zip 9** — only crates.io releases were read.
- **Rust 1.99.1's eligibility instant** depends on which publication time counts for a toolchain (channel manifest 2026-10-15T17:39:23Z, as read by the review, or GitHub release 2026-10-16T04:47:44Z); static.rust-lang.org was not read in this run. D3 (a) makes it a P03b question. *Rev 2: settled* — D3 (a) counts a toolchain's age from the later of its publication times, so P03b may land from 2026-10-23T04:47:44Z; the channel manifest was read since by modernization-plan rev 2.4's pass (HTTP 200, last-modified 2026-10-15T17:39:23Z, `rustc 1.99.1 (58cf8f9c3 2026-10-13)`).

**JS**
- **Advisories beyond npm's bulk endpoint and GitHub's global database for the direct pins.** The transitive JS tree was not checked against GHSA; the GHSA query that was made was outside the legs' allowed endpoints (§3.6). The dompurify advisory count (two or three) is unsettled.
- **Part III's JS additions** (M69's @rolldown/plugin-babel, babel-plugin-react-compiler, @babel/core 7.29.7, @types/babel__core; oxlint / oxlint-tsgolint; knip): registry peer and date facts only, not installed or gated.
- **Pre-release newer majors** (betas or RCs of vite, react, typescript and others) were not surveyed.
- **npm deprecation flags** were read for the JS direct targets only (none deprecated); the transitive `bun.lock` entries were not checked for deprecation.
- **Linux and Windows JS:** their optional platform binaries' install and the gates there; CI's Node on the ubuntu runner image after its 26.04 move (2026-10-19…11-19) — jsdom 30.1.2's floor `^24.15.0` is exactly the local Node, and Bun does not enforce `engines` at install, so a mismatch shows only at test time.
- **Whether Dependabot's bundled Bun 1.4.2 accepts `packageManager: bun@1.4.3` and the v2 lock** (P35, judged after the merge); Bun 1.4.3's own release notes (the GitHub page links bun.com, outside the allowed hosts).
- **Where vitest 5.0.4's now-default `fsModuleCache` writes** — *closed by rev 2* (§8.3): `<repo root>/node_modules/.vitest-cache`, and a `.vitest-secret-token` under `$HOME/Library/Application Support/vitest`.
- **@tanstack/react-query 5.104.2's change notes:** no GitHub release exists for its tag.

**Process notes**
- crates.io answered HTTP 429 for a while (shared IP across legs); the Rust leg's full-lock dating needed three tries, and a backoff fetcher filled the JS leg's date source. `bun x tauri info`'s crates.io lookups were rate-limited.
- Two stray registry requests carried public names only: the npm lookup ran once for the workspace's own name `openalpaca-gui` (a 404, retried three times), and a zsh unsplit variable sent one malformed npm URL listing public package names, re-run under bash.
- The scratch targets to delete when done: `rust/target` 3.9 GB, `rust/target-variant` 3.9 GB, `rust/target-gui` 772 MB (`$S/rust/logs/93-scratch-sizes.log`); the locks, logs and date caches are the record.

---

## 8. Combined tree (rev 2)

Rev 1 measured the halves apart (§1, §7). This section records one tree carrying both, measured on 2026-10-17 from 07:28:41Z (00:28:41 PDT) to 07:36:55Z by a separate step, in one detached worktree of the prep tip `7e702582` (`refs/prep/deps-wave-2026-10-12`): the Rust leg's `final.lock` as `Cargo.lock`, P03 (`.cargo/config.toml` deleted), `tauri-plugin-dialog = "=2.8.3"`, P27a's `opener = "0.9"` with `cargo update -p opener`, the JS leg's §6.2 pins (P31, P32, P31a's JS half), P33's plugin swap with its one-line `vite.config.ts` import change, root `packageManager: bun@1.4.3` (P42), and the root `bun.lock` written from scratch. Its `Cargo.lock` is therefore not `final.lock` but rev 1's opener variant (sha256 `8fb2bb51…4048`, byte-identical to `$S/rust/locks/variant-opener.lock`), and its `bun.lock` is the JS leg's (`9e3542e5…7f73`, byte-identical to `$S/js/logs/90-bun.lock`). Seven paths differ from the prep tip: `.cargo/config.toml` deleted; `Cargo.lock`, `Cargo.toml`, `apps/openalpaca-gui/package.json`, `apps/openalpaca-gui/vite.config.ts`, `bun.lock` and `package.json` changed (`39-combined-tree-state.log`). The prep tip's floors that C3 names were left as written — `rustls` `"0.23"`, `sha2` `"0.11"`, `zip` `"9.0.2"`, `keyring-core` `"1.0"` (`$C/records/combined.Cargo.toml:148`, `:156`, `:164`, `:170`) — and the lock holds rustls 0.23.46, sha2 0.11.1, zip 9.0.3 and keyring-core 1.0.1 under them, so §6.1's floors change no resolution here.

Every confined command ran under `env -i` with `HOME`, `TMPDIR`, the Bun, npm and XDG caches and `OPENALPACA_HOME_STORE` / `OPENALPACA_CONFIG_DIR` in scratch, the real `RUSTUP_HOME` and `CARGO_HOME`, and `CARGO_INCREMENTAL=0`; no `OPENALPACA_MASTER_KEY`, `APPLE_*`, `CARGO_TARGET_DIR` or `CARGO_BUILD_*` reached a command whose log does not name it. Rust 1.99.0 (`b940084d7 2026-09-28`, from `rust-toolchain.toml`), Bun 1.4.3, Node 24.15.0 (`01-env-sanity.log`). No daemon or app was started; the worktree was removed at the end and the repository stayed at `6d3e47fc` (`00-worktree-add.log`, `40-worktree-remove.log`). The registry checks used HEAD's `lockdates.py`.

**The evidence is in the session scratchpad and is not archived** (about 7.5 MB of logs, records and tools). Every log below is under

`C = /private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/4b1ee495-f087-45a9-9fa8-4325993552c8/scratchpad/d1/combined`

in `$C/logs/` (each confined command's log opens with its command, cwd, HEAD and environment and ends with its exit code); the locks, manifests, provenance files, dist hashes and `combined-tree.diff` are in `$C/records/`.

### 8.1 The gates

| Gate | Result | Log |
|---|---|---|
| Tree state: P03, `=2.8.3`, `final.lock` as `Cargo.lock`; `cargo +1.99.0 metadata --locked` | PASS (exit 0), the lock unchanged; the tree's diff equals the Rust leg's `90-worktree-diff.patch` but for git's index lines | `10-rust-p03-p31a-edits.log`, `11-metadata-locked-final-lock.log`, `12-tree-vs-rust-leg.log` |
| P27a: `opener = "0.9"`, `cargo +1.99.0 update -p opener --verbose` | PASS (exit 0): opener 0.8.5 → 0.9.0, the only version moved (765 → 765 packages, 0 added, 0 removed); the lock byte-identical to rev 1's `variant-opener.lock`, so that variant reproduces from `7e702582`; 21 dependency edges re-pointed (§8.3) | `13-p27a-opener-edit.log`, `14-p27a-cargo-update-opener.log`, `15a-lockdiff-final-vs-opener.log`, `15b-…`, `15c-…` |
| DU-23's freshness gate (`lockdates.py --gate --max-date 2026-10-14T19:53:00Z`) | PASS: `final.lock` → combined, 1 entry moved, opener 0.9.0 (2026-09-29T03:32:47Z), eligible; prep lock → combined, 27 added or moved, all eligible; every entry against an empty parent, 754 of 754 eligible, 11 workspace members undated | `16a-gate-final-to-combined.log`, `16b-gate-prep-to-combined.log`, `16c-gate-every-entry.log` |
| JS state: the JS leg's hunks for `apps/openalpaca-gui/package.json`, `vite.config.ts` and the root `package.json` (`git apply --include`) | PASS: applied cleanly; the pins are §6.2's | `20-js-pins-apply.log` |
| The root `bun.lock` from scratch: `rm bun.lock`, `bun install` under a scratch-only bunfig whose `minimumReleaseAge` = 214,689 s (the install time minus the max date; 2 s drift); the committed bunfig restored byte-identical | PASS (exit 0): 138 packages; `bun.lock` byte-identical to the JS leg's | `21-bun-install-root.log`, `21-install-age.txt`, `22-bunlock-vs-js-leg.log` |
| C5: `lockdates.py --pins npm --max-date 2026-10-14T19:53:00Z` over every name@version of `bun.lock` | PASS: `lockfileVersion` 2, 219 entries, 218 pairs eligible, 0 too young | `23-bunlock-freshness.log` |
| CI shape: `bun install --frozen-lockfile` from `apps/openalpaca-gui` under the committed bunfig (604,800 s) | PASS (exit 0): no change | `24-bun-install-frozen-ci-shape.log` |
| `bun run check` (TypeScript 7.0.2, both tsconfigs) | PASS (exit 0) | `25-bun-check.log` |
| `bun run test` (Vitest 5.0.4, jsdom 30.1.2) | PASS: **121 files / 1,551 tests**, 8.61 s | `26-bun-test.log` |
| `bun run format:check` (prettier 3.9.10) | PASS (exit 0) | `27-bun-format-check.log` |
| `bun run build` (`tsc -b`, Vite 8.3.4) | PASS: 306 modules; `dist/` 35 files / 1,059,491 B, 18 JS chunks / 777,639 B, CSS `index-BzoiVLez.css`, main chunk `index-lfDATJe7.js`; every file byte-identical to the JS leg's `dist/` | `28-bun-build.log`, `29-dist-record.log` |
| The sidecar, staged by hand before any shell build: `bun run prepare:sidecar` (dev profile, the host; no `CARGO_TARGET_DIR`, so the daemon builds into the worktree's `target/`) | PASS (exit 0): `cargo build -p openalpacad --locked` 53.49 s, 0 compiler warnings (the known proc-macro-error2 2.0.1 future-incompatibility note only); staged sha256 `db390ae4…4926`, 211,485,232 B, its `.sha256` checks; provenance: dev, `aarch64-apple-darwin`, rustc 1.99.0, `cargo_lock_sha256` `8fb2bb51…4048` | `30-prepare-sidecar-by-hand.log`, `31-stage-record-before-tauri-build.log` |
| G-GUI: `cargo +1.99.0 check -p openalpaca_gui --locked`, `CARGO_TARGET_DIR` and `CARGO_BUILD_BUILD_DIR` both `target/gui` | PASS: 0 warnings; the whole 2026-10-14 family compiled — tauri 2.12.4, tauri-build / -codegen / -macros 2.7.3, tauri-runtime(-wry) 2.12.2, tauri-utils 2.10.2, tauri-plugin 2.7.2, tauri-plugin-fs 2.7.1, tauri-plugin-dialog 2.8.3, with wry 0.57.0 and tao 0.37.1; the lock unchanged | `32-ggui-check.log` |
| G-GUI: `clippy -p openalpaca_gui --all-targets --locked`; `fmt -p openalpaca_gui --check` | PASS: 0 warnings, 0 errors; 0 hunks | `33-ggui-clippy.log`, `34-ggui-fmt-check.log` |
| `bunx tauri --version` | PASS: `node_modules/.bin/tauri` → `@tauri-apps/cli`, tauri-cli 2.12.3; nothing fetched | `35a-bunx-tauri-version.log` |
| A debug app bundle: `bunx tauri build --debug --bundles app` from `apps/openalpaca-gui`, neither `CARGO_TARGET_DIR` nor `CARGO_BUILD_BUILD_DIR` exported (A2-11) | PASS (exit 0) on the second run — the first refused `CI=1` (§8.3); the `beforeBuildCommand` hook ran `prepare-sidecar` as Tauri's (`TAURI_ENV_DEBUG` true, dev profile, the host triple, no `cargo --target`), found the daemon fresh and re-staged the same bytes; the dist rebuilt byte-identical; the shell built and bundled at `target/gui/debug/bundle/macos/openalpaca-gui.app` | `36-tauri-build-debug-app.log`, `36a-tauri-build-debug-app-CI1-refused.log` |
| G-ident, debug variant | PASS: five files, one sha256 `db390ae4…4926` (211,485,232 B) — cargo's artifact `target/debug/openalpacad`, the staged `src-tauri/bin/openalpacad-aarch64-apple-darwin`, its `.sha256` (recorded before `tauri build`, unchanged by the hook's re-stage), tauri-build's copy `target/gui/debug/openalpacad` (a different file from cargo's, so no build is compared with itself) and the bundled `Contents/MacOS/openalpacad`; `Cargo.lock` and `bun.lock` unchanged by `tauri build`; markers derived from the lock (rmcp 3.5.1; libsqlite3-sys 0.38.2, whose `sqlite3.h` says 3.53.2): `rmcp-3.5.1` = 19, `rmcp-0.16.0` = 0, `3.53.2` = 1; 35 of 35 `dist/` paths embedded in the shell | `37-g-ident.log` |
| The pair (DU-26) on the combined locks | PASS: tauri 2.12.4 shares its minor with `@tauri-apps/api` 2.12.4; tauri-plugin-dialog 2.8.3 (requirement `=2.8.3`) equals `@tauri-apps/plugin-dialog` 2.8.3; `@tauri-apps/cli` 2.12.3 is on 2.12; one copy of each in each lock; the family adds swift-rs 1.0.9 and static_vcruntime 3.0.0 | `38-pair-check.log`, `38b-tauri-family.log` |
| Home writes: `find -newermt` from 07:28:41Z, after the worktree's removal | PASS: 0 new entries in `~/.bun`, `~/Library/Caches/bun`, `~/.npm/_logs`, `~/.npm/_cacache`, `~/.cargo/bin`, `~/.rustup` and `~/Library/Application Support/vitest` (`~/.cache/bun` absent); `CARGO_HOME` gained one registry index file and its `.global-cache` stamp, cargo's own cache, no crate downloaded; `~/.openalpaca` never read, listed or stat'ed — 16 of 16 confined commands log `HOME` and `OPENALPACA_*` in scratch, no logged command or script names it, no daemon or app started | `41-home-writes.log`, `42-cargo-home-informational.log` |

### 8.2 The bundle

A debug-profile app bundle of the combined tree, built by tauri-cli 2.12.3 from the install: Tauri reports 231.66 MiB; `du -sk` 237,232 KiB; 242,914,196 apparent bytes in four files — `Contents/MacOS/openalpacad` 211,485,232 B, `Contents/MacOS/openalpaca_gui` 31,329,520 B, `Contents/Info.plist` 993 B, `Contents/Resources/icon.icns` 98,451 B. `Info.plist`: `com.tangyunpei.openalpaca-gui`, version 0.1.0, executable `openalpaca_gui`. Both binaries are Mach-O arm64 carrying only the linker's ad-hoc signature (flags `adhoc,linker-signed`): nothing signed them and nothing touched the keychain. It was never opened or launched, and it was deleted with the worktree; its hashes and listing are in `37-g-ident.log`. This is G-ident's **debug** variant, G-bundle leg A's shape — no release bundle, `package-macos.sh` run or `libexec/` copy was made, so E5's release G-ident is still to run.

### 8.3 Failures and findings

- **`CI=1` is refused by tauri-cli 2.12.3** (the one failure). The first `tauri build` exited 2: `error: invalid value '1' for '--ci'`, `[possible values: true, false]` (`36a-…`). The harness had copied `CI=1` from the JS leg's environment; the CLI reads `CI` as its `--ci` flag. Re-run with `CI=true`, it passed. Nothing in the repository sets `CI=1` for a `tauri build`; the G-base driver sets it for its bun gates and runs no `tauri build`, so a G-bundle or E5 command that copies the driver's environment sets `CI=true` or leaves it unset.
- **`cargo update -p opener` re-points 21 dependency edges.** It moved one version, opener 0.8.5 → 0.9.0, and added and removed nothing, so the freshness gate rightly saw one entry; but 21 crates now name another version already in the lock (`15a-…`): windows-sys 0.52.0 or 0.60.2 → 0.61.2 in 18 (Windows-only edges; tempfile also moves getrandom 0.3.4 → 0.4.3), cssparser-macros syn 2.0.119 → 3.0.9, num_enum_derive proc-macro-crate 1.3.1 → 3.5.0, and bstr loses regex-automata. Those crates compile against other versions than in `final.lock`. C3's "moves only opener in the lock" holds for versions, not for edges; whether P27a's lock-change body names them is the author's call.
- **tauri-cli's JS version check checks nothing under Bun.** "Looking up installed tauri packages to check mismatched versions" runs `npm list @tauri-apps/api …`, which exited 1 under Bun's `node_modules` layout (`$C/npm-logs/2026-10-17T07_34_38_145Z-debug-0.log`: `verbose exit 1`), so the CLI compared no JS versions; the DU-26 pair check is the only guard. The same family as rev 1's "not installed!" in `bun x tauri info` (§3.5). The npm debug log landing in the scratch `npm_config_logs_dir` shows G-09's variables working.
- **Where Vitest 5.0.4 writes** (closes §7's item). Its now-default `fsModuleCache` writes `<repo root>/node_modules/.vitest-cache` (8.2 MB; the root's `node_modules`, not `apps/openalpaca-gui`'s) (`26b-vitest-cache-location.log`), and it writes `$HOME/Library/Application Support/vitest/.vitest-secret-token` (37 B, mode 0600) (`26c-vitest-home-write.log`) — here under the scratch `HOME`. With the real `HOME` that is `~/Library/Application Support/vitest`, a directory the G-base driver's home-writes check (`home_writes()`, whose bun gates run with the real `HOME`) does not list. Node's compile cache went to `TMPDIR`.

### 8.4 What the combined tree does not establish

- **The workspace suite, check and clippy on this exact lock.** They were not run here, nor by any leg: rev 1's 5,126 / 0 / 17, check 0 and clippy 0 are of `final.lock`, without opener 0.9, and its opener variant ran the suite only with serde-saphyr 2.0 on top. On this lock Rust compiled only as the daemon (`cargo build -p openalpacad`, through `prepare:sidecar`, 0 warnings) and the shell (G-GUI). The figure 5,126 passed is expected for it, not measured.
- **Anything at runtime:** no launch, no G-bundle leg A or B, no IPC between `@tauri-apps/api` 2.12.4 and the 2.12.4 runtime, no WKWebView rendering of the Lightning CSS output, no confined in-app Start.
- **A release bundle:** no `--release` build, `package-macos.sh`, `libexec/` copy or E5.
- **Linux and Windows** (DU-33), and **the cut day**: §7's reproducibility bullet applies unchanged.

### 8.5 What it changes

- §1's first qualification and §7's first bullet: the two halves have been built, gated and debug-bundled together, in scratch — not as a gate of record. G-base's run at the cut instant and E1–E8 on the wave's own tip are still the gates of record, and every figure here is their expectation.
- §7's Vitest item is closed (§8.3); its opener and runtime items are narrowed.
- For the plan: G-bundle and E5 set `CI=true` or leave `CI` unset for `tauri build`; P27a's body may name the 21 re-pointed edges; the G-base driver's home-writes check misses Vitest's `~/Library/Application Support/vitest` unless its bun gates run with a scratch `HOME`.
