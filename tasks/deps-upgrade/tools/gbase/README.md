# G-base — the dependency wave's re-cut, re-run at the branch point

**Status: written 2026-10-11 (plan-rev3 §5.1 item 6a; Phase 0 of the programme, no dependency version moved). It has NOT run the real sequence yet — that run belongs to the day of the cut, 2026-10-17 or later (DU-23 (a)), before P01.** What has run: `bash -n`, `--dry-run` against `c30e41df`, `--self-test` (the three failure modes, on a toy workspace, with `cargo +1.93.0`), `--edits-only` (the eight steps' edits and the three patches reproduce `recut-source-only.diff` byte for byte on an export of `c30e41df`), and `git apply --check` of each patch against that tree.

`gbase.sh` is the named gate **G-base** (plan-rev3 §5.2): from one canonical scratch root `T=$(realpath "$(mktemp -d)")` it exports the base commit (`git archive <base> | tar -x -C $T/src`) and runs the **eight steps** of the 2026-10-10 re-cut in order — each step its anchored manifest edits, its source hunks, its cargo commands, a `cargo check --workspace --all-targets --exclude openalpaca_gui` into `$T/target` and a lock snapshot compared with the shipped one — then the full suite, the docs check, the four bun gates and G-GUI. **It exits non-zero at the first failed command**; nothing is recorded-and-continued (the defect of the September drivers the audits rejected). Every log is under `$T/logs/`; the last line prints the lock differences and the test totals.

## Files

| File | What it is |
|---|---|
| `gbase.sh` | The driver (bash 3.2-compatible — macOS `/bin/bash`). |
| `lockdiff.py` | Two `Cargo.lock` files as `(name, version)` sets; prints `name: old -> new`, `+ name ver`, `- name ver`, labels the expected differences, exits 1 on an unexpected one. |
| `lockdates.py` | Under `--dates` only: for each version a difference introduced, the crates.io `created_at` (User-Agent `openalpaca-deps-check`, one request a second). Informational, never a gate. |
| `split-recut-diff.py` | How `patches/` were made from `recut-source-only.diff` (reproducible; asserts every hunk is placed). |
| `recut-source-only.diff` | The whole wave's source changes against `7295f91b` (45,876 B, 28 files, no lock), cut on 2026-10-10 — the source of the split. `c30e41df`'s code equals `7295f91b`'s for every file in it (`git apply --check` of the whole diff passes on `c30e41df`). |
| `patches/step-4-secrets.patch` | 9 files: `crates/openalpaca_llm/src/keys/key_encryption/{mod,tests}.rs` and the eleven `format!("{:x}", <digest>)` sites of P10 (`content_io.rs`, `update_persona/common.rs`, `tests/common/eval.rs`, `hot_reload.rs` ×4, `managers/mcp.rs` ×1, `memory_tools/tests.rs` ×2, `file_edit_tests.rs`). |
| `patches/step-6-rmcp.patch` | 13 files: `crates/openalpaca_mcp/src/{client,error,lib}.rs`, `tests/filesystem_e2e.rs`; core `tools/mcp/bridge.rs`, `tools/config/annotations.rs`, `tools/registry/{capabilities,tests}.rs`, `tools/builtins/mod.rs` (the five annotation helpers), `security/sandbox/tests.rs`, `tests/mcp_integration.rs`; daemon `managers/mcp.rs` (the `Implementation::new` hunk), `routes/tools/tests.rs`. |
| `patches/step-8-config.patch` | 3 files: `crates/openalpaca_core/src/middleware/skill/{mod,renderer,tests}.rs`. |
| `locks/recut-lock-*.lock` | The per-step snapshots the re-cut measured at `7295f91b` (`00-head`, `01-toolchain`, `02-update`, `02-update-held`, `03-storage`, `04-secrets`, `05-documents`, `06-rmcp`, `07-web`, `08-config`, `99-final`, `99-final-held`). `00-head` = `01-toolchain` (the pin changes no lock); `08-config` = `99-final` (the un-held final); `99-final-held` is the final lock **with P04's hold** (rev 3.1). |
| `hold-footprint.txt` | The 58 package names whose versions the ten-crate Tauri hold changes — `lockdiff.py --names locks/recut-lock-99-final.lock locks/recut-lock-99-final-held.lock`. |

Steps 1, 2, 3, 5 and 7 have no source hunks: the diff's 28 files are the four manifests (`Cargo.toml`, `rust-toolchain.toml`, `crates/openalpaca_core/Cargo.toml`, `crates/openalpaca_mcp/Cargo.toml` — reproduced by the driver's edits, never patched) plus the 24 files of the three patches above (`split-recut-diff.py` lists them; `managers/mcp.rs` is counted in two patches).

**The one hunk-level split.** `apps/openalpacad/src/managers/mcp.rs` carries a P10 digest site (`content_hash`) and a P19 rmcp hunk (`Implementation::new`). Split by path it would break a step either way: sha2 0.11 does not compile the old `{:x}` formatting (step 4's check), and rmcp 0.16 has no `Implementation::new` (step 4's check again, from the other side). So the `{:x}` hunk is in `step-4-secrets.patch` and the other in `step-6-rmcp.patch`; both apply against the base tree in either order, and `--edits-only` proves the whole sequence reproduces the diff.

## How to run on the cut day

Prerequisites: `rustup` with the wave's toolchain installed (`cargo +1.99.0 --version` answers; the driver installs nothing), a Bun 1.4.2 binary (plan-rev3 §5.1 item 2 — never the owner's `~/.bun`), network access for `cargo update` and `bun install`, Python 3.9+, perl, git, tar. Point `TMPDIR` at a scratch volume with room for two target directories (the workspace minus the shell is ≈ 2.7 GiB cold, the suite's test binaries more; `modernization-plan.md` §1). Nothing runs inside the repository: the export under `$T/src` is what is edited, built and tested. Every cargo call is `nice -n 10 env CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=$T/target cargo +<toolchain> …`, one at a time, output to a file.

```sh
cd <the repository>
G=tasks/deps-upgrade/tools/gbase
TMPDIR=/path/to/scratch bash $G/gbase.sh --self-test --toolchain 1.99.0        # the three failure modes still caught
TMPDIR=/path/to/scratch bash $G/gbase.sh --base <branch point> --bun <bun 1.4.2> --dry-run   # pre-flight + the plan, exit 0
TMPDIR=/path/to/scratch bash $G/gbase.sh --base <branch point> --bun <bun 1.4.2> --dates     # the run (≈ 30-60 min cold)
```

`--base` is the commit the wave's first commit will sit on (plan-rev3 §5.1 item 6: "the exact branch point"); `--toolchain` defaults to 1.99.0 (DU-2); `--steps N` runs steps 1..N and stops before the final phase; `--strict` makes an unexpected lock difference a failure (below); `--with-stepbacks` runs the DU-23 (b) step-backs after the hold (off by default: a cut on or after 2026-10-17 takes the measured set); `--dates` looks the differing versions up on crates.io at the end; `--repo` names another repository to export from. Record the result — the exit code, the RESULT line, the lock differences with their dates, the counts and `$T/logs` — in `tasks/todo.md` Chunk 25 (item 7), as plan-rev3 §5.1 item 6a asks. Run it again whenever the base moves.

**The pre-flight runs before any cargo** (so a moved base fails in the second second, not the twentieth minute): the toolchain and Bun answer, every shipped file is present, the export is made, every anchor matches **exactly once** — the 20 manifest lines always, and the 17 JS anchors (16 `package.json` lines, the `vite.config.ts` import) when the run reaches the final phase, 37 in all, and each of the three patches passes `git apply --check` against the pristine export. A base that already carries one of the diff's files changed — P05 edits `key_encryption/tests.rs`, for example — fails here with the patch's name; the remedy is to re-split from a diff against that base (`split-recut-diff.py`), not to force the patch.

### The eight steps

| Step | Plan | Manifest edits (anchored whole-line replacements) | Source hunks | Cargo, then the snapshot compared with |
|---|---|---|---|---|
| 1 toolchain | P03 | `rust-toolchain.toml` `channel` → the toolchain; `Cargo.toml` `rust-version` → its major.minor | — | `check --locked …`; the lock must stay byte-identical to the base's; `recut-lock-01-toolchain.lock` |
| 2 refresh + hold | P04 | — | — | `update`; then `update -p <crate> --precise <ver>` for the ten Tauri crates in the measured order — `tauri-plugin-dialog 2.7.3`, `tauri-plugin-fs 2.5.2`, `tauri 2.11.6`, `tauri-build 2.6.3`, `tauri-runtime-wry 2.11.4`, `tauri-runtime 2.11.3`, `tauri-macros 2.6.3`, `tauri-codegen 2.6.3`, `tauri-plugin 2.6.3`, `tauri-utils 2.9.3` (DU-26 (a)); under `--with-stepbacks` the six step-backs (`toml@<locked> 1.1.6`, `toml_edit@<locked> 0.25.15`, `uuid 1.27.0`, `serde_json 1.0.151`, `tokio-util 0.7.19`, `signal-hook 0.4.4` — the `name@version` spelling derived from the lock at run time, since the bare `-p toml` is ambiguous); `check`; `recut-lock-02-update-held.lock` |
| 3 storage | P06–P08 | `rusqlite 0.40.2`, `lru 0.18`, `sysinfo 0.39.6` | — | `check`; `recut-lock-03-storage.lock` [hold] |
| 4 secrets | P09–P12 | `base64 0.23.1`, `sha2 0.11`, `aes-gcm 0.11`, `keyring 4.2` (`v1`, with its three-line comment) | `step-4-secrets.patch` | `check`; `recut-lock-04-secrets.lock` [hold] |
| 5 documents | P15, P16, P18 | `infer 0.22`, `pdf-extract 0.12`, `zip 8.6` (deflate only), `calamine 0.36` | — | `check`; `recut-lock-05-documents.lock` [hold] |
| 6 MCP client | P19 | `crates/openalpaca_mcp/Cargo.toml` `rmcp 3.5.0` | `step-6-rmcp.patch` | `update -p rmcp --precise 3.5.0` **twice** (the first resolves the changed manifest and lands on 3.5.1 [M-today]) and `update -p rmcp-macros --precise 3.5.0`; the lock must then read `rmcp 3.5.0` / `rmcp-macros 3.5.0`; `check`; `recut-lock-06-rmcp.lock` [hold] |
| 7 web + CLI | P25–P27 | `tower-http 0.7.1`, `tokio-tungstenite 0.30.0`, `rustyline 18` | — | `check`; `recut-lock-07-web.lock` [hold] |
| 8 config | P29, P30 | `toml 1.1`; `serde_yaml` → `serde-saphyr 1.3` (root, with its comment; core `.workspace`) | `step-8-config.patch` | `check`; **`recut-lock-99-final-held.lock`** — the step-8 lock is the wave's final lock |

### The lock comparison, and what "expected" means

After each step the lock is copied to `$T/locks/step-N.lock` and compared with the shipped snapshot. **Equal → ok.** Different → every differing package is printed as `name: old -> new` (`+`/`-` for added/removed) for a human, and the whole text is appended to `$T/logs/lock-differences.txt`. Two classes are expected and labelled, and are never failures:

- **`[hold]` at steps 3–7.** The shipped snapshots of those steps are the **un-held** re-cut's (the hold was measured afterwards, rev 3.1, and only `02-update-held` and `99-final-held` exist as held snapshots), so the regenerated, held lock differs from them by exactly the hold's footprint — the 58 names of `hold-footprint.txt` (the Tauri crates, wry, tao, webview2-com, the `windows` 0.61 family, brotli 8, cfb 0.7.3, `infer 0.19.0`, the five `unic-*`, the `toml 0.9.12` / `toml_datetime 0.7.5` / `winnow 0.7.15` trio …). The filter is by name, not by version, because the same hold shows different version sets at different steps (`infer` is `{0.16, 0.19}` at step 3 and `{0.19, 0.22}` at the end). Steps 2 and 8 compare with held snapshots and carry no hold filter — step 8's exact comparison is the backstop for the name-based filter's imprecision.
- **`[step-back]` under `--with-stepbacks`**: the six DU-23 (b) packages, at every step from 2 on.

Everything else is **unexpected**. Under DU-23 (a) — the cut on or after 2026-10-17 — an unexpected difference is expected to be **a release dated after 2026-10-10** (plan-rev3 §5.1 item 6: "the regenerated lock equals that file, or every difference is a release dated after 2026-10-10 and is listed with its date and its crate"); `--dates` prints the crates.io date beside each, and the human records the list. **`--strict`** turns any unexpected difference into a failure — the mode for a reproducibility check, not for the cut-day run. One more source of a few-package difference that is not a new release: `recut-lock-99-final-held.lock` was cut in one shot (the whole diff applied to a fresh export, `recut-lock-02-update-held.lock` copied in, then the three rmcp steps), not step by step, and cargo's resolver can land a stepwise path on a different compatible version — a finding for the author to read, not a driver defect.

### The final phase (`--steps all`)

1. **The full suite**: `cargo +<toolchain> test --workspace --exclude openalpaca_gui --no-fail-fast` with `OPENALPACA_HOME_STORE=$T/test-store`, `OPENALPACA_CONFIG_DIR=$T/test-store/config`, `HOME=$T/test-home`, and `RUSTUP_HOME` / `CARGO_HOME` at their real values (captured before `HOME` is overridden, or `cargo +<toolchain>` is not found). The `test result:` lines are summed; the orphaned `sleep 3600` children the `managers::mcp` tests leave (ppid 1, started inside the run) are listed and killed. Pass = rc 0 and 0 failed. Anchor: 5,081 / 0 / 16 on the re-cut [M-today]; this run is the suite's first on the held lock.
2. **`python3 -I scripts/gen_api_docs.py --check`**: expected rc 1 with **exactly** `docs/api/crates/openalpaca_mcp.md` stale (P19's `lib.rs` hunk changes the re-exports; P19/P21 regenerate it). rc 0, or any other file, is a failure.
3. **The JS set and the four bun gates**: the P31 + P32 + P33 pins as anchored `package.json` edits (react/react-dom/@types 19.3.0, @tanstack/react-query 5.104.1, dompurify 3.4.16, marked 18.1.0, tailwind-merge 3.7.0, @testing-library/dom 10.4.2, user-event 14.6.7, @types/node 26.6.5, jsdom 30.1.2, prettier 3.9.10, vite 8.3.4, vitest 5.0.3, `@vitejs/plugin-react-swc` → `@vitejs/plugin-react` 6.1.2 with the `vite.config.ts` import) — **not P31a's `@tauri-apps/*` pins**, because the Rust side is held at `tauri 2.11.6` (step 2) and the pair moves together in P31a (R20); the root `bun.lock` removed and `bun install` run from the root (DU-13 (a): a `lockfileVersion: 2` lock from scratch); then, in `apps/openalpaca-gui`, `bun run check`, `bun run test`, `bun run format:check`, `bun run build`. Bun writes only under `$T` (`BUN_INSTALL`, `BUN_INSTALL_CACHE_DIR`, `BUN_RUNTIME_TRANSPILER_CACHE_PATH`, `XDG_CACHE_HOME`). Anchors: 121 files / 1,551 tests [M-today, stage B2].
4. **G-GUI**: an empty placeholder sidecar at `apps/openalpaca-gui/src-tauri/bin/openalpacad-<host triple>` (tauri-build errors when it is absent; removed afterwards), `mkdir -p apps/openalpaca-gui/dist` (the build above fills it), `cargo check -p openalpaca_gui --locked` and `cargo clippy -p openalpaca_gui --all-targets --locked` with `CARGO_TARGET_DIR=$T/target-gui`; the lock must be unchanged. Anchor on the held lock: check 0 errors, 0 warnings [M-today, rev 3.1: `held-05-ggui-check-held.log`]; clippy was not run on it before — this run is its first.

The last line: `RESULT (<time>): lock differences: …; cargo test: P passed / F failed / I ignored; vitest: …; G-GUI: …; logs: $T/logs`.

### Exit status

- **0** — every step and gate passed; lock differences, if any, were printed (and under `--strict` there were none).
- **1** — the first failure: a cargo command (its log's last 30 lines are shown), an anchor matching 0 or 2+ times, a patch that does not apply, a lock that `--locked` changed, `rmcp` not at 3.5.0 after step 6, a failed test, the docs check's set, a bun gate, G-GUI, or an unexpected lock difference under `--strict`. The message names the step and the log.
- **2** — a usage error or a missing prerequisite (no `--base`, `cargo +<toolchain>` not installed, no `--bun` when the final phase runs, a bad `--steps`).

## The other modes

- **`--dry-run`**: the pre-flight (export, anchors, patches, toolchain, Bun), then the plan — every edit, every patch's files, every command with its log name, every comparison — and exit 0; the export is removed, the pre-flight logs stay under `$T/logs`.
- **`--edits-only`**: the export with the eight steps' edits and patches applied (and, with `--steps all`, the JS edits), no cargo. This is how the split was proved on 2026-10-11: `--edits-only --steps 8` on an export of `c30e41df`, `diff -r` against a second export with `recut-source-only.diff` applied whole — identical trees.
- **`--self-test`**: a toy workspace (two path-dependent crates, no registry, `--offline`) under `$T`, built once into `$T/target` with the given toolchain, proving (i) a failing cargo command stops the driver with a non-zero exit and nothing after it runs (a sentinel file after the command never appears), (ii) an anchor matching zero or two times stops it and leaves the file unchanged, (iii) the lock comparison prints `a: 0.1.0 -> 0.1.1` and goes on, and under `--strict` the same difference stops it — plus the controls (a passing check, an anchor matching once, equal locks). Logs: `$T/logs/selftest-*.log`. Run with the toolchain that is installed (`--toolchain 1.93.0` before P03 moves the pin). On 2026-10-11 the self-test caught one real defect in the driver before it shipped: the summary line's `moved:` count was read by a regex that also matched inside `removed:`, so every difference read as none — parsed field-wise since.

## Scope — what G-base is and is not

G-base re-runs **the measured re-cut**: the manifests' final state as `recut-source-only.diff` has it and the eight-step lock procedure of plan-rev3 §5.1 item 6, compared with the measured snapshots. Commits of the wave that are **not** in that measured state are authored by hand per §5.3 and are not part of this gate: P01/P01a/P01b/P02 (CI and repository), P05 (the format pins), P13, P14 (the fixtures), P17/P17a, P19's protocol-offer pin and its `docs/api` regeneration, P23/P28 (optional), P24 (the CORS test), **P27a (`opener 0.9` — `opener = "0.8"` is a context line of the diff)**, P31a (the Tauri pair), P34–P40. The driver changes no dependency version that the re-cut did not.

## Known limits

- bash 3.2 (macOS) and GNU bash alike; `ps -o pid= -o ppid= -o command= -A` for the orphan sweep (macOS and Linux). Paths with spaces in `TMPDIR` or the repository are not supported.
- The lock comparison's hold filter is by package name (above); step 8's exact held comparison bounds the imprecision.
- `lockdates.py` needs the network and crates.io's API; without them it prints `(date unavailable)` and the run still exits 0.
- A patch is cut at `7295f91b`'s code: a base whose copy of one of the 24 patched files changed fails the pre-flight; re-split against it.
