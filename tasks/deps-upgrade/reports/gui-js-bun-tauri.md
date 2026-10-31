# GUI JavaScript packages, Bun, and the Tauri JS/Rust pairing: upgrade research

Researcher group: **GUI JS packages, Bun, Tauri JS/Rust pairing**. Date: 2026-09-25.
Worktree: `SP/wt-deps` (detached at `caf4152e`). SP = `/private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/4b1ee495-f087-45a9-9fa8-4325993552c8/scratchpad`.
Evidence tags: **[measured]**, **[our-code]** (file:line), **[dep-source]**, **[docs]**. If a claim has no tag, it is my judgement.

## 1. Verdict

* **The whole JS set upgrades with zero source, config or test edits.** That includes the two majors (Vite 7.3.6 → 8.3.1, Vitest 4.1.11 → 5.0.2) and React 19.3. Only `package.json` pins and `bun.lock` change. All four CI gates pass at every stage: 97 test files, 1165 tests, 0 files for Prettier to reformat. [measured]
* **The committed `bun.lock` carries 10 advisories (7 high, 3 moderate).** All of them are in dev tooling pulled in by vite/vitest (nanoid, postcss, picomatch, rollup). Either fix clears all 10, and each is measured: re-resolving the lock on the Vite 7 set, or moving to Vite 8.
* **Tauri's own version guard does not protect this repo.** Under Bun it cannot see any JS package, so the major.minor rule between `tauri` and `@tauri-apps/api` (and between each plugin's crate and its npm package) has to be kept by hand. [measured + dep-source] Today and after the bump every pair is in step: tauri 2.11.x with api 2.11.1, and tauri-plugin-dialog 2.7.x with plugin-dialog 2.7.3.
* **CI already runs Bun 1.4.2; local is 1.3.13.** `oven-sh/setup-bun@v2` has no `bun-version` input, so CI takes the latest Bun. [measured, CI log] If a lock is re-resolved from scratch with Bun 1.4, it is written as `lockfileVersion 2`, and Bun 1.3.x cannot read that format. [docs] Decide which Bun version to use before any lock is re-resolved.

## 2. What I ran (logs in `SP/logs/gui-*`)

| Stage | Command(s) | Result |
|---|---|---|
| A baseline | `bun run check / test / format:check / build` in `apps/openalpaca-gui` | check 0, format 0, build 0. First `test` run **rc 1**: one timeout, `ChatView.test.tsx` "reaches past the first page…" took 7031 ms against the 5000 ms limit, at load average 66.8. Re-running that file alone gave 116/116, and re-running the full suite gave **1165/1165** at load 29.9 (`gui-A-baseline-test-run2.log`). This is load-induced flakiness that predates any bump. |
| B minor/patch | 14 exact pins edited, `bun install` | All 4 gates rc 0, 1165/1165. Prettier 3.9.9 wants **0 files** reformatted. Install warns `incorrect peer dependency "@csstools/css-parser-algorithms@4.0.0"`: bun kept 4.0.0 while css-calc 3.4.1 wants ^4.0.1. Lock: +58/−56 lines. |
| B2 minor + fresh lock | scratch copy, `bun install --lockfile-only` from no lock, then `--frozen-lockfile`, 4 gates | All rc 0, 1165/1165. 87 transitive entries move (esbuild 0.27→0.28, rollup 4.57→4.63, postcss 8.5.6→8.5.28 …). Bundle byte-identical to B (same hashes). `bun audit`: **0**. |
| C majors | + `vite 8.3.1`, `vitest 5.0.2`, `bun install`, **no config edits** | All rc 0, 97 files / 1165 tests. The one new message comes from `@vitejs/plugin-react-swc`: "We recommend switching to `@vitejs/plugin-react`…". It is printed on `vitest run` and the dev server. |
| C2 majors + fresh lock | `rm bun.lock && bun install` in the worktree (Bun 1.3.13 → lockfileVersion 1), 4 gates | All rc 0, 1165/1165. dist hashes identical to C. `bun audit` 0. Fixes the csstools peer warning. **This is `patches/gui-all.diff`.** |
| C3 option | scratch copy: `@vitejs/plugin-react-swc 4.3.3` → `@vitejs/plugin-react 6.1.1` (one import line in `vite.config.ts`) | All rc 0, 1165/1165. **dist hashes identical to C.** The recommendation warning is gone. Lock drops 16 packages (`@swc/core` plus 12 platform binaries, `@swc/counter`, `@swc/types`, the plugin) and adds 1. The dev server still injects the React-refresh preamble. |
| Dev server | `vite --port 14231` (a spare port; 1420 left alone) | Vite 8.3.1 was ready in 177 ms. `/src/main.tsx`, `/src/App.tsx` and `/src/styles.css` all returned 200, and the `$RefreshSig$` hooks are present. |
| Render | `vite preview` of the B and C builds on :14233 and :14232, Browser pane (Chromium) at 1280×800 | Chat view visually identical. Settings and Memory lazy chunks load. 0 console errors. **Not tested in WKWebView/WebView2/webkit2gtk.** |
| Audit | `bun audit` on baseline, B, B2, C, C2 | baseline 10 (7 high, 3 mod); B 10; B2 0; C/C2 0 |
| Tauri | `bun run tauri info` (RUSTUP_TOOLCHAIN=1.98.1); `npm list @tauri-apps/api … --json --depth 0` | `tauri info` reads tauri **2.11.5 from the root `Cargo.lock`**, and lists `@tauri-apps/api` and `@tauri-apps/plugin-dialog` as **"not installed!"**. `npm list` exits 1 and prints `(empty)`. |
| Tauri crates | `cargo +1.98.1 update -p tauri -p tauri-plugin-dialog`, then `cargo +1.98.1 check -p openalpaca_gui` and `clippy -p openalpaca_gui --all-targets` (target `SP/target-198`, dummy gitignored sidecar at `src-tauri/bin/`, since removed) | tauri 2.11.5→2.11.6, dialog 2.7.2→2.7.3, fs 2.5.1→2.5.2 (3 crates only). check rc 0, **0 warnings**, 185 s. clippy rc 0: 0 warnings in `openalpaca_gui`, 2 `let_and_return` in `openalpaca_storage` (`artifacts/mod.rs:1860`, `:1945`, not this group). |
| CI evidence | `gh run view --job 107353132405 --log` (main, 2026-09-23) | setup-bun@v2 downloaded **Bun 1.4.2**. `bun install --frozen-lockfile` on our v1 lock succeeded. The runner is ubuntu-24.04 image 20260907.300. There is a warning that `actions/checkout@v4` targets Node 20 and is forced onto Node 24. |
| Probes | node-shebang bin under `bun run`; `packageManager: "bun@1.4.2"` under Bun 1.3.13 | `bun run` executes `#!/usr/bin/env node` bins with the **real Node on PATH** (v24.15.0 locally), and with Bun only when no node is on PATH. Bun 1.3.13 ignores `packageManager` silently. |

## 3. Per-package findings

| Package | From → To | Kind | Finding (evidence) |
|---|---|---|---|
| react, react-dom | 19.2.8 → 19.3.0 | minor | The client production bundle grows: main chunk 377.27 → 406.99 kB (+8.85 kB gzip) on Vite 7. `react-dom-client.production.js` is 536,016 → 625,168 bytes [measured, npm pack]. 19.3's behaviour changes: Transitions render independently, StrictMode double-invokes effects during *hydration*, uSES/Activity fix, `innerHTML` is not re-set when unchanged, Trusted Types pass-through, Suspense context propagation [docs: react.dev/blog/2026/09/09/react-19-3]. Our app uses none of transitions, hydration, Activity, form actions or ViewTransition. It does use `StrictMode` + `createRoot` (`src/main.tsx:23-26`), `lazy`/`Suspense` (`src/App.tsx:46-86`), uSES via zustand/TanStack, and `dangerouslySetInnerHTML` in `DocumentPreview.tsx:139` and `MediaPreview.tsx:217` [our-code]. Tests pass [measured]. |
| @types/react, @types/react-dom | 19.2.18/19.2.5 → 19.3.0 | minor | `tsc --noEmit` and `tsc -b` clean [measured]. |
| @tanstack/react-query (+query-core) | 5.102.8 → 5.103.2 | minor | Code diff with comments stripped: hydration `dehydratedAt` is now required (SSR only, unused), a fix in `queryCache.remove`, a stricter `isPlainObject` (structural sharing), and a length guard in `partialMatchKey` [dep-source, npm pack diff]. No behaviour we rely on changes. |
| marked | 18.0.11 → 18.0.14 | patch | Around 20 CommonMark/GFM edge-case fixes that **change HTML output**: setext headings, tabs in lists and code, reference-label case folding, GFM protocol autolinks, empty code blocks, and invalid HTML tag names [docs: GitHub releases]. Used only by the artifact `DocumentPreview` (`DocumentPreview.tsx:20-21`). The chat transcript uses its own renderer [our-code]. The effect users see is limited to markdown edge cases in document previews. |
| dompurify | 3.4.14 → 3.4.16 | patch | 3.4.15: XML clobbering hardening. 3.4.16: `IN_PLACE` fixes, and the build moved to rolldown (dist `purify.es.mjs` 129 → 84 kB). No change to defaults [docs]. It sanitizes the markdown and HTML artifact previews with the default config (`DocumentPreview.tsx:21`, `MediaPreview.tsx:178`) [our-code]. This is a security-relevant sanitizer, so take it now. |
| tailwind-merge | 3.6.0 → 3.7.0 | minor | Tests and build pass. CSS output is identical to baseline on Vite 7 (same CSS hash in B) [measured]. |
| jsdom | 30.0.1 → 30.1.1 | minor | Test-only. engines `^22.22.2 \|\| ^24.15.0 \|\| >=26`, the same as 30.0.1 [measured, npm view]. |
| @testing-library/dom, user-event | 10.4.1/14.6.6 → 10.4.2/14.6.7 | patch | Tests pass [measured]. |
| @types/node | 26.4.0 → 26.6.2 | minor | Only in `tsconfig.node.json` (`vite.config.ts`, `scripts/`). Vite 8 peer `^20.19 \|\| >=22.12`, Vitest 5 peer `^22 \|\| >=24` [measured]. |
| prettier | 3.9.6 → 3.9.9 | patch | `format:check` passes. **0 files** need reformatting [measured]. |
| @tauri-apps/cli | 2.11.4 → 2.11.5 | patch | The only change is updater-signature version binding (`requireSignedVersion`) [dep-source: `node_modules/@tauri-apps/cli/CHANGELOG.md`]. We have no updater plugin (`src-tauri/Cargo.toml`, `capabilities/default.json`) [our-code]. |
| **vite** | 7.3.6 → **8.3.1** | major | Rolldown + Oxc replace esbuild + Rollup. Lightning CSS becomes the default CSS minifier. `build.target` defaults rise, `rollupOptions` is renamed `rolldownOptions`, and CJS default-interop changes [docs: vite.dev/guide/migration]. Our `vite.config.ts` uses none of the renamed or removed options, and sets `build.target: "es2022"` explicitly [our-code, `vite.config.ts:31-35`]. **No config change needed** [measured]. Output: JS total 726,970 → 723,749 bytes, 19 → 18 chunks with different chunking, and `index.html` gains 3 `modulepreload` links. CSS 48,039 → 49,457 bytes: see §4. Node floor `^20.19 \|\| >=22.12`, unchanged from 7.3.6 [measured, npm view]. 8.3.1 was published 2026-09-24. |
| **vitest** | 4.1.11 → **5.0.2** | major | Breaking items that could hit us: `clearMocks` on by default, nested `vi.mock` throws, unawaited `.resolves/.rejects` fail, `expect.poll` rejects on timeout, `toThrow("")` semantics, fake timers mock `Temporal`, jsdom global assignments propagate, and no parent-dir config lookup [docs: vitest.dev/guide/migration]. Grep: 51 `vi.mock` all at top level, 2 `.resolves/.rejects` both awaited, 0 `toThrow("")`, 0 `expect.poll`, 0 `Temporal`, 3 fake-timer files [our-code, grep]. **All 1165 pass unchanged** [measured]. Node floor is now `^22.12 \|\| ^24 \|\| >=26` (Node 20 is dropped) [measured, npm view]. **5.0.2 was published today** (5.0.0 on 2026-09-03). |
| @vitejs/plugin-react-swc | 4.3.3 (current) | — | It supports Vite 8 (peer `^4…^8`). Under Rolldown it prints the "switch to `@vitejs/plugin-react`" advice unless swc plugins are used or `disableOxcRecommendation` is set (`index.js:150`) [dep-source]. Swapping to `@vitejs/plugin-react 6.1.1` (peer `vite ^8`) is measured to be bundle-identical (C3). This is an owner choice, not forced. |
| @tailwindcss/vite, tailwindcss | 4.3.3 (current) | — | Peer `vite ^5.2 \|\| ^6 \|\| ^7 \|\| ^8` [measured]. Its own `lightningcss` stays at 1.32.0 as a nested copy, while Vite pulls 1.33.0 [measured, lock]. |
| typescript 7.0.2, zustand 5.0.15, clsx 2.1.1, tailwind-variants 3.3.1, @fontsource/* 5.3.0, @testing-library/react 16.3.3, @testing-library/jest-dom 7.0.1, prettier-plugin-tailwindcss 0.8.1, @tauri-apps/api 2.11.1, @tauri-apps/plugin-dialog 2.7.3 | current | — | `npm view <pkg> version` equals our pin for every one of them. Peers are satisfied (jest-dom `vitest >=0.32`, RTL `react ^18\|\|^19`) [measured]. After the full bump `bun outdated` prints an empty table [measured]. |

## 4. Output that users see: CSS under Vite 8

`SP/logs/gui-css-B-vs-C.diff` compares the Vite 7 CSS (esbuild minify) with the Vite 8 CSS (Lightning CSS minify) after both are split on `;{}`. It has 147 diff lines [measured]. Most are whitespace. The rest:

* `*,:before,:after,::backdrop{…}` is split into `*,:before,:after{…}` plus `::backdrop{…}` with the same declarations.
* Duplicated `-webkit-text-decoration:inherit` and `-webkit-text-decoration-color` prefixes are added.
* `border-radius:3.40282e38px` becomes `2147483647px` (the `rounded-full` sentinel).
* `outline-offset:-2px` becomes `calc(2px * -1)`.

My reading: every change is semantically equivalent, and the prefixes only widen support. That reading is not measured. The Chromium render matched pixel-for-pixel by eye. **Not checked in WKWebView (macOS), WebView2 or webkit2gtk**, which are the webviews the app ships in. A `tauri dev` session would check it, but it spawns the daemon against `~/.openalpaca`, so I did not run it (hard rule). If the owner wants the old minifier, `build.cssMinify: 'esbuild'` would require esbuild as a peer again.

## 5. Tauri JS/Rust pairing

**The rule** [dep-source: `tauri-cli-2.11.5/src/info/plugins.rs:34-41,141-167`, `build.rs:147-156`, `dev.rs:137-143`]:

* It pairs `tauri` with `@tauri-apps/api`, and each known `tauri-plugin-<x>` with `@tauri-apps/plugin-<x>`. A pair is *mismatched* when **major or minor** differ. Patch drift is allowed.
* On a mismatch, `tauri build` **fails** unless `--ignore-version-mismatches` is passed, which downgrades it to an error log. `tauri dev` only logs an error, from a background thread.
* `@tauri-apps/cli` and `tauri-build` are not paired with anything.
* Crate versions come from `<cargo metadata workspace_root>/Cargo.lock` (`helpers/cargo_manifest.rs`, `cargo_manifest_and_lock`). Measured: `tauri info` reports 2.11.5, the root lock's value. The stale tracked `apps/openalpaca-gui/src-tauri/Cargo.lock` (tauri 2.9.5, untouched since `2d5e847b`) is read by neither cargo nor the CLI.
* For Bun, npm versions come from `npm list <pkgs> --json --depth 0` run in the frontend dir (`helpers/npm.rs:302-312`). A non-zero exit yields an empty map (`:316-318`).

**What happens here.** `npm list` in `apps/openalpaca-gui` exits 1 with `(empty)`: the Bun isolated-linker workspace is not visible to npm. `tauri info` shows the JS packages as "not installed!" [measured]. So the check compares nothing and always passes. **The guard is a no-op in this repo.** A future drift, such as `cargo update` moving tauri to 2.12 while `@tauri-apps/api` stays at 2.11.x, would build silently.

**Versions:**

| Pair | Today | After this plan |
|---|---|---|
| `tauri` crate ↔ `@tauri-apps/api` | 2.11.5 ↔ 2.11.1 (ok) | 2.11.6 ↔ 2.11.1 (ok; api 2.11.1 is latest) |
| `tauri-plugin-dialog` ↔ `@tauri-apps/plugin-dialog` | **2.7.2** ↔ 2.7.3 (ok, patch drift; the brief's "2.7.3 both" is not what the lock says) | 2.7.3 ↔ 2.7.3 |
| `@tauri-apps/cli` | 2.11.4 (unpaired) | 2.11.5 |

* **tauri 2.11.6** fixes GHSA-w28w-mhc8-qvjv: queued channel payloads and large invoke responses could be read by other webviews [docs: GitHub release tauri-v2.11.6]. We run a single window (`tauri.conf.json` `app.windows` has one entry) [our-code], so our exposure is small, but take it now.
* **dialog 2.7.3**: Android/Gradle fixes and docs. **fs 2.5.2** (transitive): `fs:*` permission-scope fixes. We grant only `core:default` and `dialog:default` [dep-source CHANGELOGs; our-code `capabilities/default.json`].
* `wry` 0.55.1 and `tao` 0.35.3 show as "outdated" (latest 0.57/0.37). They cannot move: `tauri-runtime-wry 2.11.4` requires `wry "0.55.0"` and `tao "0.35.0"`, which are 0.x caret ranges [dep-source].

## 6. Bun, Node and the lockfile

* **Local and CI Bun differ.** Local is 1.3.13. CI floats and ran **1.4.2** on 2026-09-23 [measured]. setup-bun resolves in this order: `bun-version` input, then `bun-version-file`, then **the repo-root `package.json` implicitly** (`packageManager: "bun@x"` or `engines.bun`) [dep-source: setup-bun `src/index.ts:25-30`, `src/utils.ts:124` at SHA `0c5077e5`]. Adding `"packageManager": "bun@<ver>"` to the **root** `package.json` pins CI with no workflow edit. Bun 1.3.13 itself ignores the field [measured], so it pins CI only and does not guard local.
* **Bun 1.4 lockfile.** New lockfiles default to `lockfileVersion: 2`, which Bun ≤1.3 cannot read. Without `--frozen-lockfile`, 1.3.x warns, discards the lock and re-resolves [docs: oven-sh/bun#28792, dependabot-core#15848]. An existing v1 lock is **not** bumped on re-save (oven-sh/bun#31602, cited second-hand in dependabot-core#15848) [docs, not measured]. CI on 1.4.2 installs our v1 lock with `--frozen-lockfile` [measured]. Other 1.4 changes that touch us:
  * `bun update` now re-resolves transitive packages too. Under 1.3.13, `bun update` with exact pins changes nothing [measured].
  * The isolated-linker store naming changed, so a one-time rewrite of the lock is possible.
  * The Node-compat target is 26.3.
  * `bun run` gains `--parallel`.
  * [docs]
* **Consequence for this plan:** my locks were produced by Bun 1.3.13 and are v1. If the owner re-resolves with Bun 1.4.x, the file becomes v2 and every local Bun must be ≥1.4 from then on. Choose the Bun version **before** committing a re-resolved lock.
* **Node actually runs the tooling.** `bun run test/build/check` execute vitest, vite and tsc with the node on PATH [measured probe]. CI's runner image 20260907.300 has **Node 22.23.2** by default [docs: runner-images Ubuntu2404 readme at that tag]. That satisfies Vitest 5 (≥22.12), Vite 8 (≥22.12) and jsdom 30.1.1 (≥22.22.2, a margin of one minor). Nothing pins Node in CI; local is 24.15.0. A runner-image default change would move it. Optional: `actions/setup-node` with `node-version: 24`.
* **`bun audit`** (present in 1.3.13): baseline **10 advisories** [measured]:
  * nanoid 3.3.11: 3 high.
  * postcss 8.5.6: 2 high, 2 moderate.
  * picomatch 4.0.3: 1 high, 1 moderate.
  * rollup 4.57.1: 1 high (path-traversal write).

  All reach us only through vite/vitest, so they are build and dev-server tooling and none ships in `dist/`. B leaves all 10. B2, C and C2 have **0** [measured].

## 7. Risks to persisted data, secrets, wire formats, user-visible behaviour

* **Persisted data:** the GUI's only client-side persistence is hand-written `localStorage`: `oa-project` (`src/stores/project.ts:22`), `oa-pane-widths` (`pane-widths.ts:24`) and `oa-pins` (`ui.ts:42`) [our-code]. No bumped library reads or writes those keys, and the webview origin comes from Tauri, not Vite. Nothing in this group touches `~/.openalpaca`, the database, notes or keys.
* **Secrets:** none handled by these packages. Tauri 2.11.6 closes a cross-webview IPC read [docs].
* **Wire formats:** the GUI↔daemon HTTP/WS JSON is hand-written in `src/lib/api/`, and no bumped package defines it. TanStack Query only caches it; its code diff shows no serialization change [dep-source].
* **What users see:**
  * React 19.3 runtime. No transitions or hydration; tests pass.
  * CSS minifier change (§4). Equivalent by reading; checked only in Chromium.
  * `marked` edge-case output in document previews.
  * DOMPurify hardening, which can only remove more.
  * Different chunking and `modulepreload` for the app's lazy views. They load in Chromium [measured].
* **Freshness:** vitest 5.0.2 is 0 days old, 5.0.0 is 22 days old, and vite 8.3.1 is 1 day old [measured, npm time].

## 8. Proposed commits (in order)

0. **`ci: pin Bun for the GUI job` (owner decision first).** Root `package.json` gets `"packageManager": "bun@1.4.2"`, which setup-bun reads implicitly. The owner runs `bun upgrade` locally (1.3.13 → 1.4.2). This changes nothing in CI today, since it already runs 1.4.2. Optionally add `actions/setup-node` with Node 24. I did not check which major of that action is current; the CI-actions group owns that. *Verify:* CI log line "Downloading … bun-v1.4.2", then `bun install --frozen-lockfile` green.
1. **`chore(gui): minor/patch JS bumps and a re-resolved bun.lock`.** 14 pins plus a lock regenerated from scratch (`patches/gui-minor-freshlock.diff`, which is v1 because it came from 1.3.13; regenerate with the Bun chosen in commit 0). *Verify:* the four gates; `bun audit` shows 0; `bun outdated` lists only vite/vitest; `bun run build` CSS hash unchanged (`index-D_fw04La.css`).
2. **`chore(gui): Vite 8 and Vitest 5`.** Two pins plus the lock re-resolved again; no config edits (`patches/gui-all.diff` is cumulative for 1 and 2). *Verify:*
   * the four gates;
   * `vite --port <spare>` smoke;
   * `vite preview` render of Chat, Settings and Memory;
   * one `bun run tauri dev` session on an **isolated** store (`OPENALPACA_HOME_STORE` set) to see the Lightning CSS output in WKWebView;
   * `bun audit`.
3. **(optional, owner) `chore(gui): @vitejs/plugin-react-swc → @vitejs/plugin-react 6.1.1`.** `patches/gui-all-plus-plugin-react.diff`: 1 import line, `@swc/core` binaries dropped, the advice line gone. *Verify:* the four gates; identical dist hashes (measured); dev-server refresh preamble.
4. **`chore(gui): tauri 2.11.6 + tauri-plugin-dialog 2.7.3`.** Cargo.lock only (`patches/gui-tauri-crates.diff`, 3 crates). It can fold into the Rust group's lock refresh, but should land **with or after** commit 1, so the dialog pair ends at 2.7.3↔2.7.3. *Verify:* `cargo check -p openalpaca_gui` and `clippy -p openalpaca_gui --all-targets` on the chosen toolchain (measured green on 1.98.1); the `tauri build` bundle is not measured.
5. **`docs: frontend versions`.** Update `README.md:208` ("Vite 7 … Vitest 4"), `apps/openalpaca-gui/README.md:14` ("Vite 7") and `CLAUDE.md:152` ("React 19.2 … Vite 7, Vitest 4") [our-code]. Optionally note the manual Tauri pairing rule in the Gotchas.

## 9. Found in passing (out of scope, not changed)

* The stale `apps/openalpaca-gui/src-tauri/Cargo.lock` (tauri 2.9.5) is read by nothing and misleads anyone who opens it.
* A tracked 463 KB Mach-O binary `rust_out` sits at the repo root (commit `96ebdc16`).
* `pnpm-workspace.yaml` (`onlyBuiltDependencies: esbuild`) is a pnpm leftover, and esbuild is no longer installed after Vite 8.
* `actions/checkout@v4` is flagged as a Node-20 action (CI warning).
* There are two `clippy::let_and_return` warnings on 1.98.1 in `openalpaca_storage/src/artifacts/mod.rs:1860,1945`.

## 10. Patches

* `SP/patches/gui-minor.diff`: B, pins plus an incremental lock (it carries the csstools peer warning and the 10 advisories).
* `SP/patches/gui-minor-freshlock.diff`: B2, pins plus a re-resolved lock. Recommended form of commit 1.
* `SP/patches/gui-all-incremental-lock.diff`: C with the incremental lock.
* `SP/patches/gui-all.diff`: **cumulative JS**, C2 (all pins plus a re-resolved v1 lock). All gates green.
* `SP/patches/gui-all-plus-plugin-react.diff`: C2 plus the optional plugin swap.
* `SP/patches/gui-tauri-crates.diff`: Cargo.lock, 3 Tauri crates.
* `SP/patches/gui-final.diff`: the full worktree state (`gui-all.diff` plus the Tauri crates). JS gates green, `cargo check -p openalpaca_gui` green on 1.98.1.
