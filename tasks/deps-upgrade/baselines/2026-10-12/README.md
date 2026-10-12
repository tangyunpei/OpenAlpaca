# Q00 — the B0 baselines (2026-10-12)

**Status: taken on 2026-10-12 on macOS (DU-33, macOS first), at `a91f4b23` — the Phase 0 tip, whose application behaviour is that of `c30e41df` (Phase 0 changed tests, scripts, CI and tooling only). plan-rev3 §7.5 is the protocol, Q00 its entry; rev 3.8 records this file.** Two tiers ran one after the other on an otherwise unloaded M5 Max (18 cores, 128 GB, macOS 27.0.1, AC power; the owner's background apps kept the 1-minute load at about 2–4), each re-checked by an independent verifier against its logs. Evidence: `tasks/deps-upgrade/evidence-2026-10-12-q00.tar.gz` (`q00/logs/a/`, `q00/logs/b/`, each scenario's raw trials and the scripts that took them, both verifiers' verdicts in `workflow-result.json`). Every timing is 5 trials; with five trials the p95 is the maximum.

## The headline finding — the GUI half needs another launcher

Launched as plan-rev3 §5.2's launcher (iii) prescribes (`sandbox-exec -f scripts/eval/memory/loopback-only.sb <bundle>/Contents/MacOS/openalpaca_gui`), **the app's webview never reaches its daemon**: in every launch checked (8 of 18) `GET /v1/status`'s `busy.connected_clients` stayed 0 and the daemon's port had no peer. The unified log shows WebKit's Networking process "Setting sandbox state flag to block network access" in all 18 of these launches and in no other app's, and its loopback connects failing with `EPERM` (177 against 21 "Network is down" in one process) — WebKit's own network policy, triggered by the UI process running under the profile. The sidecar itself is confined as intended (`sandbox_check(network-outbound) = 1` for the shell and the daemon, the daemon a child of the shell), and the two-request proof passed. Consequences: the shell and WebKit memory/CPU rows below were measured with a **disconnected** webview and are not a GUI baseline for Q10; and **E5's step 4 uses the same launcher**, so its GUI checks would fail the same way. Recommendation (rev 3.8): for any GUI check, start the daemon first under launcher (i) on the isolated store, then launch the app **unsandboxed** with the same `OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR` — the shell reuses a live daemon rather than spawning one, so the only process with outbound reach beyond loopback stays confined; keep launcher (iii) for the cold daemon-ready figure only.

## Results

| §7.5 / Q00 item | Measured | Conditions and notes |
|---|---|---|
| Shell dependency graph (`cargo +1.99.0 tree -p openalpaca_gui -e normal --locked`) | **245** distinct `(name, version)`; **60** under `openalpaca_storage`; **226** without it; **18** only through it; storage standalone **58** | Reproduces plan-rev3 §7.2 exactly; resolution only |
| Rust baselines (G-base, cited) | `cargo test` **5,095 / 0 / 17**; clippy **78**; `gen_api_docs --check` clean; rustfmt drift 1,585 hunks in 303 files | The gate at `1379b184` (rev 3.6), not re-run |
| Frontend baselines | vitest **1,551** tests in 121 files (cited); `dist/` **1,020,842 B** in 36 files; JS **740,645 B** in 19 chunks; CSS 48,427 B; entry chunk **395,009 B** (gzip 116,823 B) | Bun 1.4.3, Vite 7.3.6, TypeScript 7.0.2; reproduces plan A10 byte for byte |
| (8) clean full build of the CI set | `CARGO_INCREMENTAL=0`: **39.8 s** (p95 39.8); `=1`: **44.2 s** (p95 44.6) | Empty `CARGO_TARGET_DIR`, registry pre-fetched, dev profile, rustc 1.93.0; target 2.0 GiB / 4,187 files (incr 0) vs 2.7 GiB / 7,976 files (incr 1) |
| (8) warm full build (every member rebuilt, every dependency fresh — `cargo clean -p` of the 10 non-GUI members) | incr 0: **24.1 s**; incr 1: **29.1 s** | `clean -p` also removes the members' incremental caches, so incr 1 ran with none |
| (8) warm, every member's root file touched | incr 0: 24.4 s; incr 1: **4.6 s** | Supplementary: the edit-rebuild an incremental developer meets |
| (8) daemon-only incremental (`touch apps/openalpacad/src/main.rs`; `cargo build -p openalpacad`) | incr 0: **9.3 s** (p95 10.4); incr 1: **1.7 s** | The first `-p openalpacad` build after a CI-set build recompiles 14 units (24.6 s): the daemon's own feature graph |
| (8) shell-only incremental (`touch src-tauri/src/lib.rs`; `cargo build` in `src-tauri/`, `target/gui`) | incr 0: **1.8 s**; incr 1: **0.94 s** | |
| (8) support-only incremental | not applicable | `openalpaca_app_support` does not exist until Q06 |
| (8) React HMR turnaround (`utime` of a leaf component → Vite's `update` message) | 5 trials: median **22 ms**, p95 115 ms; 20 trials: median 78 ms, p95 114 ms | Bimodal (~12 ms or ~112 ms) — Q10 compares a 20-trial (or larger) series; the 20-trial run overlapped an external load spike (3.5 → 10.5); every touch is a 2-module update (Tailwind re-emits `styles.css`) |
| (1) daemon-ready, the release daemon alone under `loopback-only.sb` | cold (fresh store): **73 ms** (p95 76); warm: **56 ms** (p95 66) | ~35 ms of it is the live Ollama model refresh on the boot path; a newly written binary's first exec adds ~0.39 s (macOS first-exec cost; the copy's provenance unlogged) |
| (3) daemon idle, 60 s after ready | phys_footprint **8.2 MB**; CPU **0.5 %** of one core; no children | `footprint`, cross-checked with `vmmap -summary` |
| Packaged build (E5 steps 1–3, throwaway identifier `com.tangyunpei.openalpaca-gui.q00`) | **G-ident holds**: bundled `openalpacad` = staged `.sha256` = cargo's artifact (`e7c82751…`); bundle 66 MiB (daemon 58.7 MB, shell 9.9 MB); pre-wave markers present (`rmcp-0.16.0`) | |
| (1) cold launch, no daemon → daemon-ready | **0.440 s** (p95 0.480); → `discovery.json` 0.406 s | Launcher (iii); the bundle's first-ever exec 0.964 s |
| (1) first shell paint, first useful content | not measured | Needs a person at the screen or an instrumented build |
| (2) launch with the daemon running | not measured | No headless signal of the webview's connection under launcher (iii) (it never connects) |
| (3) idle foreground 60 s — whole app | combined footprint **139 MiB** (p95 172); CPU 1.1 % | **Disconnected webview** — not a GUI baseline. Per process (medians): Web Content 64.7 MiB, shell 35.5, GPU 24.3, daemon 8.5, Networking 6.0 |
| (3) hidden / minimized 60 s | not measured | Needs a window action |
| (4) one streamed ~2,000-token answer | daemon peak **11.5 MiB** (8.0 before); shell flat 32.3 MiB | 2,267 output tokens in 61.7 s on `qwen3.8:27b-mlx`; driven through the API, so the window did not render it live (the GUI renders only streams it starts); a background learning call overlapped the tail. Dropped frames: not measured |
| (5) 100-message conversation scrolled; (6) three previews | not measured | Need a person |
| (7) close/reopen cycles | **10/10 quit/relaunch cycles, the daemon survived each** (same `instance_id` and pid, reparented to launchd, never a second spawn) | A quit/relaunch variant; closing the window itself not exercised |

## What a person still owes the baseline

First paint and first useful content, launch with the daemon running, idle and hidden memory with a **connected** webview, the streamed answer rendered in the window with dropped frames, the 100-message scroll, three previews, and window close/reopen — all under the recommended launcher above, on the same commit, before P03 (they are comparable only on this application commit).

## Notes

- The fixture: onboarding done in one attended turn (`BOOTSTRAP.md` gone), one scratch git repository registered as a project, 20 notes through `POST /v1/memories` (24 in the snapshot: a learning pass added 4), no 100-message conversation (scenario 5 cannot be driven headlessly).
- CLAUDE.md's "a cold rebuild of the CI set takes 144 s (118 s with incremental off)" is modernization-plan §1.3's **sum of three commands** (bare build, workspace build, `test --no-run`); a single CI-set build is the 39.8 s / 44.2 s above.
- Side effects outside the scratch area: three temporary WebKit directories under `/var/folders/…/T/com.apple.WebKit.*+openalpaca_gui-<hash>`, keyed by the ad-hoc code-signing identifier the owner's own builds share (so a throwaway bundle id does not isolate them); the throwaway's `~/Library` entries were removed. Two agents (tier A and tier B's verifier) read the **metadata** of `~/.openalpaca` (`ls -ld`, `find -newer`, `stat`) to check nothing changed there — no content read, nothing written; a breach of the rule, disclosed.
