# Group report: document extraction and file sniffing

zip 2.4.2 → 8.6.0 · calamine 0.26.1 → 0.36.1 · pdf-extract 0.8.2 → 0.12.1 (lopdf 0.34.0 → 0.42.0) · infer 0.16.0 → 0.22.0

Worktree `SP/wt-docs` (detached at `5e5ac317`), target `SP/target-198`, scratch `SP/scratch-docs/`.
`SP` = `/private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/4b1ee495-f087-45a9-9fa8-4325993552c8/scratchpad`.
Patch: `SP/patches/documents.diff` (Cargo.toml + Cargo.lock only; **compiles** on 1.98.1 and on 1.93.0).

Evidence tags: **[measured]** I ran it · **[our-code]** file:line in the worktree · **[dep-src]** a path under `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/` · **[docs]** changelog/advisory/URL · **[not established]**.

---

## 1. Verdict in one table

| crate | from → to | our code changes | result | recommendation |
|---|---|---|---|---|
| pdf-extract (+lopdf) | 0.8.2 → 0.12.1 (lopdf 0.34 → 0.42) | none | **Fixes a daemon-killing DoS** (RUSTSEC-2026-0187, reproduced: SIGABRT); decrypts AES PDFs; stops raw `println!` spam into `daemon.log`; **introduces one text regression** (Form XObject font scope, reproduced) | upgrade-with-care, **first**, as its own commit |
| calamine | 0.26.1 → 0.36.1 | none | Fixes RUSTSEC-2026-0194/0195 (quick-xml 0.31 → 0.41); two whole-sheet drops fixed; cell text changes (whitespace trim, rich text, CRLF, `_x000D_`) | upgrade-with-care, together with zip |
| zip | 2.4.2 → 8.6.0 | none (manifest: `default-features = false, features = ["deflate"]`) | reading DOCX/PPTX/iWork byte-identical on all fixtures; minimal features drop 23 lock entries incl. 3 C-backed crates; off-spec bzip2/lzma entries stop reading | upgrade-now (with calamine) |
| infer | 0.16.0 → 0.22.0 (also in openalpaca_core) | none | **Refuses some text uploads that pass today** (`%PDF` anywhere in the first 1 KiB; lines starting `AC1015` etc.); accepts some audio/HEIF/prefixed-PDF uploads refused today | upgrade-with-care; owner decides whether to keep today's text/* verdicts |

All four compile with **zero source changes**: `cargo +1.98.1 check -p openalpacad --all-targets` exit 0, 0 errors, 1 warning line (the `proc-macro-error2` future-incompat note, from teloxide→aquamarine, not this group); `-p openalpaca_core --all-targets` exit 0, 0 warnings, 0 errors; `cargo +1.93.0 check -p openalpacad --all-targets` exit 0, 0 warnings, 0 errors **[measured]**. So this group can land before, after or without the toolchain bump.

---

## 2. What I ran

| # | command (all prefixed `nice -n 10 env CARGO_TERM_COLOR=never CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=…`) | log (`SP/logs/`) | result |
|---|---|---|---|
| 1 | edit root `Cargo.toml` (4 lines), `cargo +1.98.1 update -p zip -p calamine -p pdf-extract -p infer@0.16.0` | docs-update.txt | "Locking 38 packages to latest Rust 1.93 compatible versions" |
| 2 | `cargo +1.98.1 check -p openalpacad --all-targets` (zip default features) | docs-check-openalpacad-1.txt | exit 0, 0 `^error`, 1 `^warning` (future-incompat note) |
| 3 | `cargo +1.98.1 check -p openalpaca_core --all-targets` | docs-check-core-1.txt | exit 0, 0/0 |
| 4 | zip → `{ version = "8", default-features = false, features = ["deflate"] }`, `cargo update -p zip --offline` | docs-update-zipmin.txt | removes 21 packages (aes 0.9, hmac 0.13, pbkdf2 0.13, sha1/sha2 0.11, digest 0.11, bzip2, lzma-rust2, zstd, deflate64, ppmd-rust, …) |
| 5 | `cargo +1.98.1 check -p openalpacad --all-targets` (minimal zip) | docs-check-openalpacad-2-zipmin.txt | exit 0, 0 errors, same 1 note |
| 6 | `cargo +1.98.1 test -p openalpacad -- extraction:: routes::files background::` | docs-test-openalpacad-1.txt | 47 passed, 0 failed (17 extraction, 27 routes::files, 3 background) |
| 7 | `cargo +1.98.1 test -p openalpaca_core --lib -- sanitizer daemon_config` | docs-test-core-1.txt | 76 passed, 0 failed |
| 8 | `cargo +1.98.1 test -p openalpaca_connectors --lib -- common::` | docs-test-connectors-1.txt | 13 passed, 0 failed |
| 9 | `cargo +1.98.1 clippy -p openalpacad -p openalpaca_core --all-targets` | docs-clippy-198.txt | exit 0; 84 warnings, **none** in extraction.rs / files.rs / files_types.rs / sanitizer (all are 1.98 lints elsewhere → toolchain group) |
| 10 | `cargo +1.93.0 check -p calamine -p pdf-extract -p zip -p infer@0.22.0 -p lopdf` then `-p openalpacad --all-targets` (target `SP/scratch-docs/target-193`) | docs-check-deps-193.txt, docs-check-openalpacad-193.txt | both exit 0, 0 warnings, 0 errors |
| 11 | `cargo-audit audit --no-fetch --db SP/tools/advisory-db -f <lock>` on the HEAD lock and on the patched lock | docs-audit-before.txt / -after.txt | before: 5 vulnerabilities (lopdf RUSTSEC-2026-0187, quick-xml 0194 + 0195, rmcp, rustls); after: 2 (rmcp, rustls — other groups). New warning after: `ttf-parser 0.25.1` unmaintained (RUSTSEC-2026-0192), pulled by lopdf 0.42 |
| 12 | `cargo tree -d`, `-i`, `-e features` before/after | docs-tree-*.txt | §5, §7 |
| 13 | comparison harness `SP/scratch-docs/cmp` (old and new crates side by side under renamed deps; `extraction_body.rs` = `apps/openalpacad/src/extraction.rs` verbatim minus `//!`; each run in its own process on a **2 MiB** thread, release build) over 40 fixtures + 754 system PDFs | docs-compare-min.txt, `SP/scratch-docs/out/` | §3–§5 |

No fixture files exist in the repository: `git ls-files` and a `find` of the main checkout for `*.pdf *.xlsx *.xls *.docx *.pptx *.zip *.ods` return nothing **[measured]**. So fixtures were built in `SP/scratch-docs/fixtures/` by `make_fixtures.py` (hand-built PDFs/XLSX targeting each changelog item), copied from macOS system files (`/Library/Documentation/License.lpdf/…/License.pdf` in English, Chinese, Japanese, Arabic; `textutil`-converted DOCX of the same licences; a `cupsfilter` PDF; a `python-docx` DOCX), and encrypted PDFs written with lopdf 0.42's own `Document::encrypt` (RC4-128, AES-128, AES-256, empty user password). Apple PDFKit (`SP/scratch-docs/pdfkit_text`) was used as an independent reference reader.

The harness lock was pinned to the workspace's transitive versions where they differed (encoding_rs 0.8.35, codepage 0.1.2, zlib-rs 0.6.7, crc32fast 1.5.1, indexmap 2.14.1). In the harness both sides share one flate2 build (zlib-rs enabled), which does not affect decompressed bytes.

---

## 3. pdf-extract 0.8.2 → 0.12.1 (lopdf 0.34.0 → 0.42.0)

**Our usage [our-code]:** one call, `pdf_extract::extract_text(path)` in `apps/openalpacad/src/extraction.rs:60`, run from the file-processing worker inside `tokio::task::spawn_blocking` (`apps/openalpacad/src/background.rs:135-139`); a panic becomes the persisted error `Extraction task panicked`. The runtime is built without `thread_stack_size` (`apps/openalpacad/src/main.rs:179-182`), so blocking threads have tokio's default 2 MiB stack (**[dep-src]** `tokio-1.53.1/src/runtime/builder.rs:662-663`). API at our call site unchanged; the encrypted-variant functions changed `password: PW: AsRef<[u8]>` → `&str` **[dep-src]** `pdf-extract-0.12.1/src/lib.rs:2246-2272`, unused by us.

**Findings, each measured on the fixtures (`SP/logs/docs-compare-min.txt`):**

1. **Security — process abort fixed.** `p06_nested_arrays_10k.pdf` (20,676 bytes, one unreferenced object of 10,000 nested arrays): old = process **killed by SIGABRT** (`thread '<unknown>' has overflowed its stack / fatal runtime error: stack overflow, aborting`); new = returns the page text `Nested arrays live in object 6`. A stack overflow aborts the whole process, so in the daemon one uploaded PDF kills `openalpacad` **[measured in harness on a 2 MiB thread; daemon crash inferred from Rust's abort-on-overflow]**. Advisory RUSTSEC-2026-0187, lopdf ≤ 0.41, patched 0.42, CVSS 7.5 **[docs]** https://rustsec.org/advisories/RUSTSEC-2026-0187.html; cargo-audit flags it on HEAD's lock and not on the patched one **[measured]**. Reachable from `POST /v1/files/upload` and from connector attachments (Telegram/Discord/iMessage via `store_attachment`, `crates/openalpaca_connectors/src/common/mod.rs:189-230`), the latter only for linked users because `TrustGate` denies `chat.respond` to unlinked principals before attachments are stored (`crates/openalpaca_core/src/security/policy.rs:58-61`, `telegram/handler.rs` Step 3 before Step 3.5) **[our-code]**. Pre-existing and unchanged: an asset whose extraction killed the daemon stays `processing` forever — the worker only picks `Uploaded` and retryable `Error` rows (`background.rs:88-105`) and nothing resets `Processing` **[our-code]**.
2. **AES-encrypted PDFs now extract.** `e01_p01_encrypted_v4.pdf` (AES-128) and `_v5.pdf` (AES-256): old error `PDF extraction failed: PDF error: the document uses an encryption scheme that is not supported`; new returns text byte-identical to the unencrypted `p01_simple.pdf`. RC4-128 (`_v2`) worked in both. PDFKit reads v2 and v4 (confirms the fixtures are valid). User-visible: such uploads go from `Error` to `Ready` with text.
3. **Regression introduced upstream — Form XObject font scope.** 0.8.2 kept the font cache local to each `process_stream` call (`pdf-extract-0.8.2/src/lib.rs:1576`); 0.12.x moved it onto the `Processor` keyed by resource name only (`pdf-extract-0.12.1/src/lib.rs:1572, 1712`) and clears it per page (`:2408`), while `Do` recurses into the same processor (`:1860-1868`) **[dep-src]**. Resource names are scoped to their own dictionary, so a Form XObject whose `/F1` differs from the page's `/F1` is decoded with the wrong font. Measured on hand-built fixtures (PDFKit gives the correct text):
   - `p02` (page text first, then `/X1 Do`): old `ABC page text` / `ZYX form text` (correct) → new `ABC page text` / **`ABC form text`**.
   - `p03` (`Do` first): old `ZYX form text` / `ABC page text` (correct) → new `ZYX form text` / **`ZYX page text`**.
   Upstream history **[docs]** https://github.com/jrmuizel/pdf-extract/commits/master: "Avoid reparsing fonts for every page" (Mar 2026) introduced the shared cache; 0.12.1's "Reset font cache per page" fixed only the cross-page half. No released pdf-extract has lopdf ≥ 0.42 without this cache (0.12.0 and 0.12.1 both require `^0.42`; 0.10.0 requires `^0.38`) **[measured from the local index cache]**. How often real documents hit it is **[not established]**: plausible for N-up/imposed/stamped PDFs where each original page becomes a Form XObject with its own `/F1`.
4. **Real-world text unchanged.** Uncapped extraction of the four licence PDFs (78,286 / 68,367 / 1,012 / 152,942 bytes) and the cupsfilter PDF is byte-identical old vs new; a sweep of 754 distinct system PDFs (2 KB–3 MB, `SP/scratch-docs/out/sweep.txt`) gave identical status, text and error for all 754 (only 2 carry > 200 bytes of text, so this is weak evidence for reading order) **[measured]**. Pre-existing, unchanged: the Japanese licence yields 1,012 bytes of mostly newlines against PDFKit's 42,433 characters.
5. **Log noise.** 0.8.2 writes diagnostics with `println!` (e.g. `missing char … in unicode map …`, `falling back to encoding …`); 0.12.1 routes them through `log` (`debug!`/`warn!`/`error!`) **[dep-src]** (diff in `SP/scratch-docs/pdf-extract-lib.diff`). The managed daemon's stdout and stderr both go to `daemon.log` (`apps/openalpaca/src/manager.rs:58-63`, `apps/openalpaca-gui/src-tauri/src/lib.rs:267-268`) and the tracing subscriber bridges `log` records (tracing-subscriber default feature `tracing-log`, `main.rs:97-100`, filter `info`) **[our-code, dep-src]**. Measured on `/System/Library/ProductDocuments/ProductGuides/ENERGY STAR.pdf`: old prints **13,698** raw stdout lines for one extraction, new prints 0; both emit the same 15 `ERROR lopdf::reader Object load error …` records at info level. Five other system PDFs: 24–510 stdout lines → 0.
6. **Unchanged failure modes.** `p04` (Form XObject with `/Filter [/ASCIIHexDecode /FlateDecode]`): both panic with `index out of bounds` (`pdf-extract-0.8.2 lib.rs:1800`, `0.12.1 lib.rs:1806`) → persisted `Extraction task panicked`. `p07` (300 bytes of junk before `%PDF`): both fail, message changes from `PDF error: Invalid file header` to `PDF error: failed parsing cross reference table: invalid start value` (persisted in `file_assets.extract_error`).
7. **Deps:** lopdf 0.42 with `default-features = false, features = ["wasm_js"]` (the feature only affects wasm) pulls aes 0.8/cbc/ecb/md-5/sha2 0.10/stringprep/ttf-parser/nom 8; MSRV 1.85 **[dep-src]** `lopdf-0.42.0/Cargo.toml:14`; pdf-extract declares no MSRV.

---

## 4. calamine 0.26.1 → 0.36.1

**Our usage [our-code]:** `open_workbook_auto(path)`, `sheet_names()`, `worksheet_range(name)`, `range.rows()`, `Data::to_string()` joined with `\t` (`extraction.rs:87-112`); a sheet whose `worksheet_range` errors is silently skipped (`if let Ok(range)`). Both XLSX and XLS MIME types go here; the format is chosen by the stored file's extension (`open_workbook_auto`, unchanged between versions **[dep-src]** `calamine-0.36.1/src/auto.rs:35-59`). API unchanged at our call sites (compiles); `vba_project` changed shape (0.32), unused. MSRV 1.88 **[dep-src]**. Depends on zip `^8.6` with `deflate` only and quick-xml `^0.41` **[dep-src]** `calamine-0.36.1/Cargo.toml:141-151`, so calamine and zip must move together or the daemon links two zips.

**Measured cell-text differences** (fixture `x03_cases_clean.xlsx`, `diff` of `cat -vet` output; `^I` = tab, `^M` = CR):

| input (shared or inline string) | old (0.26.1) | new (0.36.1) | changelog |
|---|---|---|---|
| `<t>  leading and trailing  </t>` (no `xml:space`) | `  leading and trailing  ` | `leading and trailing` | 0.36 trims unless `xml:space="preserve"` |
| `<t>Plain </t><r><t>Bold run</t></r>` | `Plain ` | `PlainBold run` | 0.35 rich runs after plain `<t>` no longer dropped (and the trim joins them) |
| `<t>line1\r\nline2</t>` | `line1\r\nline2` | `line1\nline2` | 0.31 XML EOL normalisation |
| `<t>a_x000D_b_x0009_c</t>` | `a_x000D_b_x0009_c` | `a\rb\tc` | 0.31 unescapes Excel escapes — a real tab now collides with our `\t` cell separator |
| `<t>\nnewline-led</t>` | `\nnewline-led` | `newline-led` | 0.36 trim |
| inline `<is><t>  inline padded  </t></is>` | `  inline padded  ` | `inline padded` | 0.36 trim |
| inline CDATA `<t><![CDATA[cdata <text> & more]]></t>` | *(empty)* | `cdata <text> & more` | 0.35 |

Unchanged (measured): `xml:space="preserve"` strings, CJK/emoji, numbers (`0.30000000000000004`, `100000000000000000000`, `42`, `-3.5`, `0.000000123456789012345`), dates/times/percent/accounting formats (both print the Excel serial, e.g. `45200`, `45200.5`, `0.75` — `Display for ExcelDateTime` writes the raw value in both, **[dep-src]** `calamine-0.26.1/src/datatype.rs:771-775`, `0.36.1:986-990`), booleans, `#DIV/0!`/`#N/A`, the 1904 epoch, sparse rows.

**Whole-sheet drops fixed (measured, bisected with `sheeterr`):** in 0.26.1 one odd cell makes `worksheet_range` fail, so our extractor drops the **entire sheet** silently: an `inlineStr` cell with a redundant `<v>` → `Err(Xlsx(CellTAttribute("inlineStr")))`; a `t="s"` cell with an empty `<v/>` → `Err(Xlsx(ParseInt(… Empty)))`. 0.36.1 reads both sheets (the redundant-`<v>` cell itself comes back empty). On `x01_cases.xlsx` the old extraction is 63 bytes (sheet 1 missing) and the new one 500 bytes.

**Not measured:** XLS (BIFF) — no generator available. Changelog items that reach our output there are panic/OOM fixes (0.29, 0.32, 0.35 BIFF5 formulas) and empty strings kept (0.36.1), which print as empty either way **[docs]** `calamine-0.36.1/Changelog.md`.

**Security:** quick-xml 0.31 (via calamine 0.26) is affected by RUSTSEC-2026-0194 (quadratic duplicate-attribute check) and RUSTSEC-2026-0195 (NsReader allocation), both fixed in quick-xml 0.41 **[docs]** rustsec.org; cargo-audit confirms both disappear with the patch **[measured]**.

---

## 5. zip 2.4.2 → 8.6.0

**Our usage [our-code]:** read-only — `ZipArchive::new`, `by_name`, `by_index`, `len`, `name`, `Read` (`extraction.rs:71-82, 119-141, 177-211`); the test helper writes a Stored archive with `ZipWriter`/`SimpleFileOptions` (`extraction.rs:293-303`). Never `extract()`. The upload archive-bomb heuristic in `InputSanitizer` parses local headers by hand and does not use the zip crate.

**Why 2 → 8** (**[dep-src]** `zip-8.6.0/CHANGELOG.md`): the project releases a major whenever cargo-semver-checks reports a break. 3.0.0 dropped the deprecated `deflate-miniz` feature and decoupled `deflate-flate2` from a backend; 4.0.0 Zip64 fix and MSRV reverts; 5.0.0 `by_path*`; 6.0.0 `FileOptions::add_extra_data` generic (breaking); 7.0.0 removed the `getrandom/hmac/pbkdf2/sha1/zeroize` and `lzma-static/xz-static` features and moved LZMA to lzma-rust2; 7.4.0 raised MSRV to 1.88; 8.0.0 Rust 2024 and removal of deprecated `DateTime` methods. None of these touch our calls.

**Yanks [measured, local index cache refreshed by `cargo update` today]:** 2.0.0–2.2.3 yanked; 2.3.0–2.4.2 **not** yanked (we are on a live version); 2.5.0, 2.6.0, 2.6.1 yanked; 7.3.0 and 7.4.0 yanked. Why 2.5/2.6 were yanked is **[not established]** (issue https://github.com/zip-rs/zip2/issues/315 has no maintainer answer in the fetched page).

**Security [docs]:** the only zip advisory, RUSTSEC-2025-0168 / CVE-2025-29787 (symlink path traversal in `ZipArchive::extract`), affects 1.3.0–2.2.x, fixed in 2.3.0 — we were never affected (no `extract`). Zip bombs: neither version caps decompressed size (zip 8's `Decompressor::Deflated` is a bare `flate2::bufread::DeflateDecoder` **[dep-src]** `zip-8.6.0/src/compression.rs:506-508`); our code reads whole entries into memory, so the bomb defence stays the sanitizer's ratio heuristic — unchanged by this bump. Hardening in between: 5.1.1 fixed a panic on an empty extended-timestamp field; 2.6.1 stopped scanning every local header on open **[docs]**.

**MSRV:** 1.88 **[dep-src]** `zip-8.6.0/Cargo.toml:14` (2.4.2: 1.73).

**Features.** 2.4.2 default = `aes-crypto, bzip2, deflate64, deflate(=flate2/rust_backend + zopfli), lzma(lzma-rs), time, zstd, xz(xz2)`; 8.6.0 default = `aes-crypto, bzip2, deflate64, deflate(=deflate-zopfli + deflate-flate2-zlib-rs), lzma, ppmd, time, zstd, xz` **[dep-src]** both Cargo.toml. With zip 8's defaults the daemon gains 21 packages (399 vs 378 in `cargo tree -p openalpacad -e normal,build`) and five new RustCrypto duplicates (aes 0.8/0.9, cipher, digest, sha1, sha2, hmac…) **[measured]**. calamine 0.36 already turns on `deflate`, so `features = ["deflate"]` is the smallest possible set; with it the daemon graph is 377 packages (−1 vs HEAD) and the lock loses bzip2-sys, lzma-sys, xz2, zstd (C code) among 23 removals **[measured]**.

What the minimal set changes **[measured, harness built twice, `--features zip8-full`]**: a DOCX whose `word/document.xml` is **bzip2**-compressed is read by zip 2.4.2 and by zip 8 with defaults, but with `["deflate"]` it reports `DOCX missing word/document.xml`; an **LZMA** entry failed in zip 2.4.2 already (`Failed to read word/document.xml`), reads with zip 8 defaults, and reports "missing" with `["deflate"]`. Stored and deflated entries (everything Office, iWork and the test helper produce) are byte-identical in all three. ECMA-376 packages allow only stored/deflate, so this only affects hand-made archives **[docs, inferred]**. To keep today's readable set exactly, use `["deflate", "deflate64", "bzip2", "lzma", "xz", "zstd"]` (re-adds 11 packages incl. zstd's C library and sha2/digest 0.11 **[measured]**; `aes-crypto` is never needed because we never pass a password).

**flate2 backend switch [measured + dep-src]:** before, flate2 in the daemon had `rust_backend` (miniz_oxide) only; after, zip's `deflate` adds `zlib-rs`/`any_zlib`, and flate2 then selects zlib-rs for **every** flate2 user in the binary (`flate2-1.1.10/src/ffi/mod.rs:69-77`). In the daemon those users are only zip and lopdf, and only decompress; decompressed bytes are backend-independent and the fixtures confirm identical text. In a whole-workspace build flate2 now appears twice in `cargo tree -d` (the GUI's build-time png keeps the default features), i.e. one extra compile unit, GUI builds only.

---

## 6. infer 0.16.0 → 0.22.0 (openalpacad and openalpaca_core)

**Our usage [our-code]:** `infer::get(data)` in `validate_magic_mime` (`apps/openalpacad/src/routes/files_types.rs:18`) for `POST /v1/files/upload` → `415 MIME_MISMATCH` / `415 MIME_UNDETECTABLE` (`routes/files.rs:134-160`), and in `InputSanitizer::validate_upload_with_image_limit` (`crates/openalpaca_core/src/security/sanitizer/mod.rs:215`), used by the upload route and by connector attachments (`store_attachment`). Detection never assigns a MIME type; it only accepts or refuses the declared one. API unchanged; MSRV 1.74; std feature pulls cfb 0.14 (was 0.7) **[dep-src]**.

**Detection changes [dep-src]** (`SP/scratch-docs/infer-map.diff`, matcher diffs): new matchers qcow2, **dwg (`image/vnd.dwg`)**, par2, bzip3, lz4; **PDF matched anywhere in the first 1024 bytes** (`infer-0.22.0/src/matchers/archive.rs:105-119`; was offset 0); MP3 frames `FF F3`/`FF F2` recognised; HEIF major brand `heix` recognised. No existing MIME string changed; zip (map line 449) is still tried before pdf (line 503), so OOXML/iWork containers are unaffected.

**Verdicts [measured]** (`SP/scratch-docs/out/infer.tsv`; route = upload route, connector = sanitizer):

| fixture (declared) | old detect → route / connector | new detect → route / connector |
|---|---|---|
| text/plain containing `%PDF-1.7` in line 2 | None → OK / OK | application/pdf → **415 MIME_MISMATCH / BLOCKED** |
| text/html containing `%PDF-1.4` | text/html → OK / OK | application/pdf → **415 MIME_MISMATCH / BLOCKED** |
| text/csv starting `AC1015,Widget,3` | None → OK / OK | image/vnd.dwg → **415 MIME_MISMATCH / BLOCKED** |
| audio/mpeg, MPEG-2 frame `FF F3` | None → 415 MIME_UNDETECTABLE / OK | audio/mpeg → OK / OK |
| audio/mp3, same bytes | None → 415 MIME_UNDETECTABLE / OK | audio/mpeg → 415 MIME_MISMATCH / **BLOCKED** |
| image/heif, brand `heix` | None → 415 MIME_UNDETECTABLE / OK | image/heif → OK / OK |
| image/heic, brand `heix` | None → 415 MIME_UNDETECTABLE / OK | image/heif → 415 MIME_MISMATCH / **BLOCKED** |
| application/pdf with 300-byte junk prefix | None → 415 MIME_UNDETECTABLE / OK | application/pdf → OK / OK (extraction then fails in both lopdf versions) |

Unchanged (measured): every real PDF/DOCX/XLSX/PPTX/iWork fixture; plain text; MPEG-1 MP3. Pre-existing and unchanged: an ordinary iPhone HEIC declared `image/heic` is refused `MIME_MISMATCH (detected image/heif)` by both versions.

Inferred consequence **[not measured]**: a client declaring `image/vnd.dwg` for a real DWG is now accepted instead of refused, and is then routed as an *image* attachment (`crates/openalpaca_core/src/orchestrator/handler_attachments.rs:23-31`), which a provider will likely reject.

**Duplicates:** HEAD has infer 0.16 (ours) and 0.19 (tauri-utils 2.9.3, GUI only); after the bump it is 0.22 + 0.19, and cfb becomes 0.7 + 0.14. The newest stable tauri-utils (2.9.3) still requires `^0.19`; only 3.0.0-alpha requires `^0.22` **[measured, index cache]**. The daemon binary itself has one infer either way.

---

## 7. The duplicate question

The brief expected two calamine and two lopdf versions; on HEAD there is **one** calamine (0.26.1) and **one** lopdf (0.34.0); only infer (0.16/0.19) and quick-xml (0.31 via calamine, 0.41 via plist→tauri) are doubled among this group's crates **[measured]** `cargo tree --workspace --target all -d`. After the patch: quick-xml collapses to 0.41, constant_time_eq collapses to 0.4; infer stays doubled (0.19/0.22), cfb becomes doubled (0.7/0.14), flate2 shows a feature split. With zip defaults instead, aes, block-buffer, cipher, crypto-common, digest, inout, sha1, sha2 would all be doubled too. `cargo tree -p openalpacad -d` lists none of this group's crates after the patch.

---

## 8. Risks

**Persisted data.** Extracted text is persisted once per upload in `file_assets.extracted_text` (`background.rs:141-153`, the write at `:148`); nothing re-extracts `Ready` rows, so existing rows keep the old text and only new uploads (and `Error` rows still within `extraction_retry_count`) see the new libraries **[our-code]**. No schema, migration, key or file layout change. The notes, the memory index and the database are untouched by this group.

**Secrets.** None of these crates handles secrets. zip's `aes-crypto` (only for encrypted archive entries) is dropped by the minimal feature set.

**Wire formats / what the model sees.** (a) Extracted text for XLSX changes as in §4 and for Form-XObject PDFs as in §3.3; it travels to LLM providers inside attachments. (b) Upload refusals: `MIME_MISMATCH`/`MIME_UNDETECTABLE` verdicts change as in §6, including the `detected '…'` part of the message. (c) Persisted `extract_error` strings change for PDFs that fail to parse (§3.6) and for off-spec DOCX (§5).

**User-visible.** Improvements: no daemon crash on nested-array PDFs; AES PDFs readable; XLSX sheets no longer vanish on odd cells; `daemon.log` loses thousands of raw pdf-extract lines per odd PDF. Regressions: some text uploads refused (§6); Form-XObject font confusion (§3.3); bzip2/LZMA-compressed DOCX entries unreadable with the minimal feature set (§5).

---

## 9. Proposed commits (in order)

Each commit: `cargo build|test --workspace --exclude openalpaca_gui`, `cargo clippy --workspace --exclude openalpaca_gui --all-targets` as CI does, plus the commit-specific checks below.

1. **`test(uploads): characterise extraction and sniffing on generated fixtures`** (tests only, on HEAD). Add in-test-generated fixtures (no binaries) to `extraction.rs` and `routes/files.rs` tests: the XLSX cases of §4 (incl. the two sheet-killing cells), a DOCX with a bzip2 entry, text/plain and text/csv bodies containing `%PDF` and `AC1015`, and a nested-array PDF test marked `#[ignore]` on HEAD (it aborts the test binary). *Verify:* green on HEAD with the old expectations.
2. **`fix(deps): pdf-extract 0.12 / lopdf 0.42 — RUSTSEC-2026-0187`** (`pdf-extract = "0.12"` + lock). Un-ignore the nested-array test; add an AES-128 test; add the Form-XObject scope test asserting today's **correct** output and mark it `#[ignore = "pdf-extract 0.12 regression, upstream issue <n>"]`, and file that issue with the p02/p03 fixtures. *Verify:* `cargo test -p openalpacad extraction::`; `cargo audit` no longer lists lopdf; the harness shows `p06` returning text.
3. **`chore(deps): calamine 0.36 + zip 8 (deflate only) — RUSTSEC-2026-0194/0195`** (together, because calamine 0.36 needs zip `^8.6`). Update the XLSX expectations to §4's new column and the bzip2-DOCX expectation (or pick the six-feature set, see decision D2). *Verify:* `cargo test -p openalpacad extraction:: routes::files`; `cargo tree -p openalpacad -i zip` shows one zip; `cargo tree -d` shows no quick-xml duplicate; `cargo audit` clean for quick-xml.
4. **`chore(deps): infer 0.22`** (+ lock). Either update the text/* sniffing expectations (accepting §6) or, if the owner keeps today's verdicts (D1), a separate `fix(uploads): …` commit that ignores a `%PDF`-not-at-offset-0 or `image/vnd.dwg` detection for declared `text/*`, with its tests. *Verify:* `cargo test -p openalpacad routes::files` and `-p openalpaca_core sanitizer`, `-p openalpaca_connectors common::`.

Commits 2–4 are independent of each other and of the toolchain bump (1.93.0 check measured green).

---

## 10. Owner decisions

- **D1 (infer):** accept that text uploads containing `%PDF` in their first KiB, or starting with a DWG version string, are refused (415 / connector block), or keep today's verdict for declared `text/*`? Recommendation: keep today's verdict with a narrow rule, since a note about PDFs is a normal thing to upload.
- **D2 (zip features):** `["deflate"]` (recommended; spec-conforming files unchanged; −23 lock entries incl. C code) or the six-feature set that keeps off-spec bzip2/xz/zstd/deflate64 entries readable.
- **D3 (pdf-extract):** accept the Form-XObject font-scope regression in exchange for the DoS fix (recommended; no release offers one without the other), report upstream, and consider a `[patch]` only if a user hits it.
