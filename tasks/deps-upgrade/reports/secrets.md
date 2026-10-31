# Secrets and hashing: sha2 0.11, aes-gcm 0.11, base64 0.23, keyring 4

Group: secrets and hashing. Worktree `SP/wt-secrets` (detached at `5e5ac317`), target dir `SP/target-198`, scratch `SP/scratch-secrets/`.
Patch: `SP/patches/secrets.diff` (Cargo.lock included, 1548 lines). **It compiles**: `cargo +1.98.1 check --workspace --all-targets --exclude openalpaca_gui` passes with 0 errors, and so does `cargo +1.93.0 check -p openalpaca_llm -p openalpaca_storage -p openalpaca_core --all-targets`.

`SP` = `/private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/4b1ee495-f087-45a9-9fa8-4325993552c8/scratchpad`. `REG` = `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f`.

Evidence tags used below: **[measured]** (I ran it, command and result given), **[our-code]** (file:line in this repo), **[dep-source]** (path under `REG`), **[docs]** (changelog or release notes URL). Where I could not establish something, the text says so.

## Summary

| Package | From → to | Code change | Measured result | Risk | Recommendation |
|---|---|---|---|---|---|
| aes-gcm (with aead 0.6, cipher 0.5, ctr 0.10, ghash 0.6, polyval 0.7) | 0.10.3 → 0.11.1 | 1 file (`key_encryption/mod.rs`, +10/−12) plus 1 test line | Stored values decrypt both ways, byte-identical ciphertext for fixed key+nonce, published NIST vectors match | low (after measuring) | upgrade-with-care: put the fixture test in first |
| sha2 (with digest 0.11, hybrid-array 0.4) | 0.10.9 → 0.11.0 | 10 `format!("{:x}", digest)` sites in 7 files (+38/−10) | Hex identical across 5,021 inputs and every idiom; owner-key and revision known-answer tests pass | low | upgrade-now (compile errors guard it, and the known-answer tests exist) |
| base64 | 0.22.1 → 0.23.1 | none | Encodings identical; accept/reject decisions identical; one error-message wording changed | low | upgrade-now |
| keyring | 3.6.3 → 4.2.0 | Cargo.toml only (features renamed) | Compiles on macOS. The Linux build no longer pulls `libdbus-sys` | medium (not measurable on a real keychain) | upgrade-with-care, in its own commit |
| blake3 | 1.8.7 | none (already the latest) | – | none | nothing to do |
| rand | 0.10.2 → 0.10.3 | none (lock refresh) | – | low | leave to the lock-refresh group |

Two points carry the owner's data:

1. **Every `secret_encrypted` value in `llm.toml` decrypts after the bump**, and values written after the bump decrypt with today's build (so a rollback is safe too). This is measured against a verbatim copy of HEAD's `key_encryption/mod.rs` and the migrated file, and in the real crate with a fixture test.
2. **Every SHA-256 hex string is byte-identical.** This covers memory owner keys (partition directory names), partition keys (`state/memory-control/<key>.json`), note revisions, `file_assets.sha256`, persona ETags and session-log hashes. The one idiom whose spelling could have changed (`{:x}`) now fails to compile on sha2 0.11 (`LowerHex` is gone from `hybrid_array::Array`), so it cannot change silently. All 10 sites were rewritten to the per-byte `{b:02x}` form that `memory_files::sha256_hex` already used, and that form is measured identical.

## What I ran

All cargo commands used `nice -n 10 env CARGO_TERM_COLOR=never CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=<dir>` with an explicit toolchain. Every output went to a file under `SP/logs/`.

| # | Command (cwd) | Log | Result |
|---|---|---|---|
| 1 | Edit `Cargo.toml` (4 requirements), then `cargo +1.98.1 fetch` (wt-secrets) | `secrets-fetch.txt` | exit 0. "Locking 65 packages to latest Rust 1.93 compatible versions". aead 0.5.2→0.6.1, aes-gcm 0.10.3→0.11.1, ctr 0.9.2→0.10.1, ghash 0.5.1→0.6.0, polyval 0.6.2→0.7.3, universal-hash 0.5.1→0.6.1, keyring 3.6.3→4.2.0. Added: sha2 0.11.0, digest 0.11.3, hybrid-array 0.4.15, crypto-common 0.2.2, cipher 0.5.2, base64 0.23.1, keyring-core 1.0.0, apple-native-keyring-store 1.0.2, windows-native-keyring-store 1.1.0, zbus-secret-service-keyring-store 1.0.1, secret-service 5.2.0, zbus 5.19.0 and its async stack. Removed: dbus-secret-service 4.1.0, rand_core 0.6.4, security-framework 2.11.1, core-foundation 0.9.4, opaque-debug 0.3.1 |
| 2 | `cargo +1.98.1 check --workspace --all-targets --exclude openalpaca_gui` (unfixed) | `secrets-check-1-unfixed.txt` | exit 101. 5 `error[...]`: E0432 `aes_gcm::aead::OsRng` unresolved (`key_encryption/mod.rs:5`), and E0277 `Array<u8, …>: LowerHex` not satisfied at `content_io.rs:18` (lib and lib-test), `database/tests.rs:1097` and `store/tests.rs:440`. Plus deprecation warnings for `Array::from_slice` (×4) and `rand_core::RngCore`. core and openalpacad were not reached |
| 3 | Mechanical fixes (below), then the same check | `secrets-check-2.txt` | exit 0. 0 errors, 1 warning (future-incompat `proc-macro-error2`, already in HEAD's lock) |
| 4 | Keyring requirement made explicit (`version = "4.2", default-features = false, features = ["v1"]`), check again | `secrets-check-3-final.txt` | exit 0. 0 errors, same 1 warning |
| 5 | `cargo +1.98.1 test -p openalpaca_llm -p openalpaca_storage` | `secrets-test-llm-storage.txt` | exit 0. llm: 245 passed. storage: 678 passed, 3 ignored |
| 6 | `cargo +1.98.1 test -p openalpaca_core --lib -- memory update_persona session_log persona` | `secrets-test-core-memory.txt` | exit 0. 476 passed, 4 ignored, 1548 filtered out, 0 warnings |
| 7 | Scratch differential program `SP/scratch-secrets/compat` (`cargo +1.98.1 run`) | `secrets-compat-run.txt` | exit 0, `ALL CHECKS PASSED` (details per package below) |
| 8 | Fixture test temporarily appended to `key_encryption/tests.rs`, `cargo +1.98.1 test -p openalpaca_llm --lib -- key_encryption`, then the file restored | `secrets-test-kat.txt` | exit 0, 14 passed, including `a_secret_written_by_the_aes_gcm_0_10_build_still_decrypts`. Not left in the patch |
| 9 | `cargo +1.93.0 check -p openalpaca_llm -p openalpaca_storage -p openalpaca_core --all-targets` (target `SP/scratch-secrets/target-193`) | `secrets-check-193.txt` | exit 0. 0 errors. The 9 warnings are the existing dead-code warnings in `openalpaca_llm/src/test_support.rs`, which appear whenever the crate is built alone without provider features |
| 10 | `cargo +1.98.1 clippy --workspace --exclude openalpaca_gui --all-targets` | `secrets-clippy.txt` | exit 0. 114 warnings, none on a line I changed. The one hit in an edited file (`key_encryption/tests.rs:64`, `needless_borrows_for_generic_args` on an untouched line) is also in the toolchain group's HEAD run (`logs/tc-A-lintcounts.txt:17`) |
| 11 | `cargo +1.93.0 fmt --all -- --check` | `secrets-cargofmt.txt` | The tree already has fmt diffs at HEAD (the toolchain group's `tc-A-fmt193.log` shows the same). None falls in a hunk I edited |
| 12 | `cargo +1.98.1 tree --workspace --exclude openalpaca_gui --target x86_64-unknown-linux-gnu -i libdbus-sys -e normal,build` | `secrets-tree-linux-dbus-after.txt` | "did not match any packages": the CI build no longer needs libdbus. For `-p openalpaca_gui` it is still pulled in by `tao 0.35.3 → dbus 0.9.12` (`secrets-tree-linux-dbus-gui.txt`). At HEAD, `libdbus-sys ← dbus ← dbus-secret-service 4.1.0 ← keyring 3.6.3 ← openalpaca_llm` (`secrets-tree-dbus-base.txt`) |

## Mechanical fixes (all in the patch)

- **aes-gcm** (`crates/openalpaca_llm/src/keys/key_encryption/mod.rs`, `tests.rs`)
  - `aead::OsRng` and `rand_core::RngCore` are gone. Key and nonce generation now use `aead::Generate::generate()` (crypto-common 0.2's getrandom `SysRng`, `REG/crypto-common-0.2.2/src/generate.rs:41-44`). This is still the OS CSPRNG, and it still panics on RNG failure, exactly as `OsRng::fill_bytes` did.
  - `Key::from_slice` and `Nonce::from_slice` (deprecated in hybrid-array 0.4, `REG/hybrid-array-0.4.15/src/lib.rs:1062`) became `try_from(&[u8])` or `From<[u8; N]>`.
  - `encrypt` and `decrypt` now take `&nonce`.
  - Judgement: the unreachable `try_from` failures are mapped to `Err(String)` rather than unwrapped. The length is already checked, so the extra branches are dead code; nothing else needed judgement.
- **sha2** (`content_io.rs:18`, `database/tests.rs:1097`, `store/tests.rs:440`, `update_persona/common.rs:8`, `tests/common/eval.rs:363`, `hot_reload.rs:43/244/279/317`, `managers/mcp.rs:2435`): `format!("{:x}", d)` became `d.iter().map(|b| format!("{b:02x}")).collect()`, which is the idiom `memory_files::sha256_hex` (`crates/openalpaca_storage/src/memory_files/mod.rs:139-147`) and `session_log/writer.rs:650-655` already use. It is purely mechanical. I deliberately did not consolidate the six duplicated helpers into one: that is a refactor, not something the bump forces.
- **base64**: nothing.
- **keyring**: nothing in code. `keyring::Entry::new`, `get_password`, `set_password`, `delete_credential` and `keyring::Error::NoEntry` exist unchanged in v4's `v1` module (`REG/keyring-4.2.0/src/v1.rs:27-103`, which re-exports `keyring_core::{Error, Result}`; `NoEntry` is at `REG/keyring-core-1.0.0/src/error.rs:40`). Only the workspace dependency line changed, because the v3 features `apple-native`, `windows-native` and `sync-secret-service` no longer exist.

## aes-gcm 0.10.3 → 0.11.1

**Our construction [our-code]** (`crates/openalpaca_llm/src/keys/key_encryption/mod.rs`):
- **Master key.** 32 raw bytes, stored as 64 lowercase hex characters in `state/.master_key` (`ensure_at`, `:94-173`, written with `create_new` and mode 0600), or supplied in `OPENALPACA_MASTER_KEY`, which wins (`:66-71`). There is **no key derivation**: the hex decodes straight into the AES-256 key (`from_hex`, `:73-85`).
- **Cipher.** AES-256-GCM, 96-bit nonce drawn from the OS RNG on every encryption, no AAD, 128-bit tag appended by the library.
- **Stored form.** `"aes256:" + base64 STANDARD (padded)` over `nonce(12) || ciphertext || tag(16)` (`encrypt`, `:175-193`; `decrypt`, `:196-218`).
- **Where it applies.** `secret_encrypted` in `llm.toml` is the default storage path (`use_keychain = false`, `apps/openalpacad/src/services/llm.rs:13-20`).

**Changes that hit us [dep-source / docs]:**
- `aead` 0.6 removed its nonce-generation APIs in favour of the `Generate` trait, and dropped the `OsRng` re-export. See `REG/aead-0.6.1/CHANGELOG.md`, "Nonce generation APIs - use `Generate` trait instead", and `REG/aead-0.6.1/src/lib.rs:16-27`.
- `generic-array` was replaced by `hybrid-array`, whose `from_slice` is deprecated.
- `Aead::encrypt` and `decrypt` take `&Nonce<Self>` (`REG/aead-0.6.1/src/lib.rs:158-190`).
- The default implementation still appends a postfix tag (`:195-213`), which is the same layout as before.
- aes-gcm 0.11 removed its `std` and `stream` features. We used neither.

**Measured compatibility [measured]** (`SP/scratch-secrets/compat`, log `secrets-compat-run.txt`). The program compiles two modules:
- `old_key_encryption.rs`: HEAD's file byte for byte (`git show HEAD:…`), minus its `//!` line and test-module declaration, linked to `aes-gcm =0.10.3` and `base64 =0.22.1`.
- `new_key_encryption.rs`: the migrated worktree file, the same way, linked to `aes-gcm =0.11.1` and `base64 =0.23.1`.

It then checks:
- **A. Cross-decryption.** 20 master keys, 10 minted by each version's `ensure_at`. For each, 500 plaintexts (10,000 in total, including Unicode, empty strings, `=`, `/`, `+` and zero-width characters): 0.11 decrypts every value 0.10 wrote, 0.10 decrypts every value 0.11 wrote, the wire length is equal, and the layout is always `12 + len + 16`.
- **B. Failure paths.** 149 failure inputs: a single-bit flip at every byte (3 bit positions each), a truncated tag, a nonce alone, a too-short payload, a missing prefix, bad base64, unpadded base64, the wrong key, and a non-UTF-8 plaintext. The outcome is identical in every case, and there are **0 message differences**.
- **C. Library level.** 3,000 random (key, nonce, plaintext, AAD) tuples: the `ciphertext||tag` bytes from 0.10.3 and 0.11.1 are identical.
- **D. Published vectors.** NIST GCM spec test cases 13 and 14 (AES-256, IV = 0^96), computed by 0.11:
  - TC13 = `530f8afbc74536b9a963b4f1c4cb738b`
  - TC14 = `cea7403d4d606b6e074ec5d3baf39d18` `d0d1c8a799996bf0265b98b5d48ab919`

  These match the published vectors as I remember them, so treat that one comparison as memory, not a lookup. It is not load-bearing, because C already compares the two libraries directly.
- **E. Randomness.** 5,000 nonces from the migrated code are all distinct. Two fresh master keys differ, and a second `ensure_at` call reads the existing key back instead of minting a new one.
- **F. Fixture.** Written by the 0.10 code under key `00..1f`:
  `aes256:HFTMjzaU0zEto0whNGd6YqMOlnKtBdUbBHMw8KubnY16SHhYQwgboFOmStXEzYAXfEtXNQavok+LGjf7` → `sk-ant-api03-test-key-1234567890`.

  The real migrated crate decrypts it too (run 8). I propose committing this as a test before the bump (commit 1).

**Risk.** Low, given the measurements. The only behavioural change is the RNG source (getrandom `SysRng` in place of rand_core 0.6's `OsRng`), and both read the OS CSPRNG. MSRV 1.85 [dep-source: `REG/aes-gcm-0.11.1/Cargo.toml`].

## sha2 0.10.9 → 0.11.0 (digest 0.11, hybrid-array)

**Call sites [our-code]:**

| Site | What the hex names | Persisted or wire? |
|---|---|---|
| `crates/openalpaca_storage/src/memory_files/mod.rs:139-175` (`sha256_hex`, `owner_key` = first 32 hex, `dir_key` = first 16, `home-<owner>`, `proj-<owner>-<dir>`); `note.rs:275` `revision_of` | Partition **directory names** `memory/owners/<owner-key>/`, snapshot file names `state/memory-control/<partition-key>.json`, note revisions inside the snapshots, `.history/<uuid>/<revision>.md` | Persisted. A change would orphan every partition |
| `crates/openalpaca_core/src/memory/service/mod.rs:1694` (listing filter fingerprint, first 8 hex, inside a cursor) | Pagination cursor | Wire, ephemeral |
| `crates/openalpaca_storage/src/content_io.rs:15` → `artifacts/mod.rs:177`, `uploads/mod.rs:96` | `file_assets.sha256` (upload dedup by hash, `uploads/mod.rs:359`) | Persisted (DB). A change would miss dedup, not lose data |
| `crates/openalpaca_core/src/tools/builtins/update_persona/common.rs:6` → `writer.rs:57` | Persona `ETag` / `If-Match` (`apps/openalpacad/src/routes/persona.rs:177`) | Wire |
| `crates/openalpaca_core/src/session_log/writer.rs:465, 596-655, 712` | Spill and snapshot hashes in session JSONL records | Persisted, informational |
| `apps/openalpacad/src/hot_reload.rs:41, 244, 279, 317`; `managers/mcp.rs:2433` | Own-write rings for config and persona watchers | In-memory only |
| Tests: `database/tests.rs:1095` (schema-42 fixture bytes unchanged), `store/tests.rs:438` (HOME_README digest tripwire), `memory_index/fixture_tests.rs:37-46, 277`, `memory_index/bench.rs:204`, `tests/common/eval.rs:360` | Test oracles | – |

`blake3` is used separately (prompt fingerprints) and does not move.

**Change that hits us [dep-source / measured].** In 0.11 the output is `hybrid_array::Array<u8, U32>`. hybrid-array 0.4.15 implements no `LowerHex` (grep of `REG/hybrid-array-0.4.15/src`, `REG/digest-0.11.3/src` and `REG/sha2-0.11.0/src` finds none). So `format!("{:x}", …)` is a **compile error** (E0277, run 2), never a silent format change. `.iter()` and `AsRef<[u8]>` still work, and the `iter` / `{b:02x}` form compiles unchanged at the sites that already used it.

**Measured equality [measured]** (compat program, "SHA" lines):
- 5,021 inputs: lengths 0, 1, 55, 56, 63, 64, 65, 119, 120, 127, 128, 129, 1000, 4096 and 65537, 5,000 random lengths up to 2 KiB, and the strings "", "abc", "test", "u_example_alice", "system" and "用户".
- On every input, 0.10's `format!("{:x}")` equals each of: 0.10 incremental `{:x}`, 0.10 `iter`, 0.10 `AsRef`, 0.11 `iter`, 0.11 chunked incremental, 0.11 `AsRef`, and 0.11 over `&Vec<u8>`.
- Known-answer tests under 0.11:
  - `sha256("") = e3b0c442…b855`, `"abc" = ba7816bf…15ad`, `"test" = 9f86d081…0a08`
  - `owner_key(u_example_alice) = 32acea7eb739b08afd959db91e46d450`, `u_example_bob = 5c357145…0e71`, `system = bbc5e661…2c2f`, `u_example_alice_reborn = 9bfb77d3…07a0`

  These are the fixture README values.
- In the real crates after the bump, these existing tests pass (run 5): `memory_files::tests::owner_key_is_first_32_hex_of_sha256_over_utf8`, `partition_keys_have_the_design_shape`, `revision_is_lowercase_hex_sha256_of_exact_bytes`, `store::tests::a_changed_home_readme_lists_the_text_it_replaces` (literal digest), `database::tests::the_schema42_fixture_is_refused_without_vec0_and_its_bytes_are_unchanged`, and the upload dedup tests. Core memory, persona and session-log tests: 476 passed (run 6). They pin literal owner keys (`memory/service/tests.rs:25`, `memory_tools/tests.rs:22-23`, `prompt_ctx/sources/memory.rs:622`) and a literal revision (`memory/service/tests.rs:1849`).

**Transitive note [measured, lock].** sha2 0.10.9 and digest 0.10.7 stay in the lock for `tauri-codegen`/`wry` (sha2) and `hmac 0.12`, `md-5`, `pbkdf2`, `sha1` (digest). So generic-array 0.14 remains through `block-buffer 0.10`, `crypto-common 0.1` and `inout 0.1`. "generic-array → hybrid-array" is complete for our own code, not for the graph. secret-service 5.2 (the Linux keyring backend) pulls sha2 0.11, aes 0.9, cbc 0.2, hkdf 0.13 and hmac 0.13 with `crypto-rust`.

## base64 0.22.1 → 0.23.1

**Call sites [our-code]:**
- `STANDARD`: `key_encryption/mod.rs:7`, which stores secrets; `update_persona/{soul,user,identity}.rs`, which decode model tool input; `orchestrator/handler_attachments.rs:227`, which encodes image bytes sent to a provider.
- `URL_SAFE_NO_PAD`: `openalpaca_storage/src/discovery/mod.rs:78`, the discovery token; `memory/service/mod.rs:1697-1705`, memory listing cursors.
- No code matches on `DecodeError` variants (grep).

**What changed [docs: `REG/base64-0.23.1/RELEASE-NOTES.md`; dep-source]:**
- New SIMD engines behind the default-on `simd-unsafe` feature.
- A clearer `InvalidLastSymbol` message.
- Custom padding symbols, and MSRV 1.71.
- `general_purpose::STANDARD` and `URL_SAFE_NO_PAD` are still `GeneralPurpose` with unchanged configs (`REG/base64-0.23.1/src/engine/general_purpose/mod.rs:337-344, 436, 472, 487-510`). `GeneralPurpose` stays scalar: its SIMD prefix hook is `|_, _| (0, 0)` (`:85-109`).

**Measured [measured]** (compat program, "B64" lines). For both engines:
- 2,000 encodes are identical, and cross-version round trips succeed.
- 200,000 random decode inputs (alphabet characters, `=` and occasional junk): identical accept/reject decisions and identical bytes in every case.
- **Only the text of `InvalidLastSymbol` changed.** For example, `"Invalid last symbol 102, offset 2."` becomes `"Invalid last symbol 0x66 ('f') at offset 2, decoded as 0b00011111."` This happened on 275 of 200,000 STANDARD inputs and 47,471 URL_SAFE_NO_PAD inputs.

That text reaches:
- `key_encryption` "Invalid base64: …", but only for a corrupted `llm.toml` value;
- the `update_persona` tool error the model sees.

The memory cursor path maps every error to a fixed sentence (`memory/service/mod.rs:1704-1707`).

**Graph note [measured, lock].** base64 0.22.1 stays for axum, hyper-util, reqwest (0.12 and 0.13), rmcp, serde_with, tokio-websockets, plist, wry and tauri-codegen. So the bump adds a duplicate base64 until those move.

**Optional (owner's call, not in the patch):** `base64 = { version = "0.23.1", default-features = false, features = ["std"] }`. This stops compiling the unsafe SIMD engines that our `GeneralPurpose` calls never use.

## keyring 3.6.3 → 4.2.0

**What v4 is [dep-source / docs].** v4 split keyring into `keyring-core` (the API) and one crate per credential store. The `keyring` crate is now a wrapper whose default `v1` feature recreates the old `Entry` API over one platform store (`REG/keyring-4.2.0/src/v1.rs:1-24, 107-124`; `Cargo.toml` features `v1 = [apple-native-keyring-store/keychain, windows-native-keyring-store, zbus-secret-service-keyring-store]`). The release notes (https://github.com/open-source-cooperative/keyring-rs/releases) say:
- 4.0.0 was "just a sample app";
- 4.1.0 "recreates the v1 (and v2/v3) functionality of providing an `Entry` object".

Hence the requirement `version = "4.2"` with the feature named explicitly. The release notes do not discuss reading credentials v3 wrote; everything below comes from source.

**How our three features map:**

| v3 feature | v4 (`v1` feature) |
|---|---|
| `apple-native` → `keyring::macos` (security-framework 3, legacy keychain API) | `apple-native-keyring-store` 1.0.2, `keychain` module (security-framework 3.7, same legacy API) |
| `windows-native` → `keyring::windows` | `windows-native-keyring-store` 1.1.0 |
| `sync-secret-service` → `keyring::secret_service` over `dbus-secret-service` 4.1 (**libdbus**, and in our build `EncryptionType::Plain` because we enabled neither `crypto-*` feature: `REG/keyring-3.6.3/src/secret_service.rs:141-143`) | `zbus-secret-service-keyring-store` 1.0.1 over `secret-service` 5.2 and **zbus** (pure Rust, `crypto-rust`, a DH-encrypted session: `REG/zbus-secret-service-keyring-store-1.0.1/src/service.rs:24-28`) |

**Linux libdbus [measured].** After the bump, `cargo tree --workspace --exclude openalpaca_gui --target x86_64-unknown-linux-gnu -i libdbus-sys` finds nothing. The CI Rust job therefore no longer needs `libdbus-1-dev` (see `.github/workflows/ci.yml:19-21` and `README.md:29`). The GUI crate still needs it on Linux through `tao → dbus`. `pkg-config` stays a build-dependency of `aws-lc-sys`, so keep installing it. This is not measured on a Linux runner.

**Does v4 find what v3 wrote?** Our entries are `Entry::new("OpenAlpaca", "llm/<provider>/<uuid>")` (`crates/openalpaca_llm/src/keys/secret_store/mod.rs:13, 46-70`; refs minted at `config/settings_service.rs:335` and `config/llm_config/migration.rs:101`). `keyring_core::Entry::new` passes service and user through unchanged (`REG/keyring-core-1.0.0/src/lib.rs:130-135`). Per platform, from source; **I did not touch any real keychain**:

- **macOS: same item.**
  - v3: `MacCredential { domain: User, service, account: user }`, written with `SecKeychain::default_for_domain(User).set_generic_password(service, account, …)` and read with `find_generic_password(Some(&[User keychain]), service, account)` (`REG/keyring-3.6.3/src/macos.rs:58, 81, 159-168, 240-251`).
  - v4: `Store::new()` → `MacKeychainDomain::User` (`REG/apple-native-keyring-store-1.0.2/src/keychain.rs:182`); `Cred { domain, service, account: user }`; the same `set_generic_password` and `find_generic_password` calls on the same login keychain (`:73, 81, 103`).
  - v4 uses the legacy file-based keychain, not the data-protection keychain (that is the separate `protected` feature, not enabled).
  - Delete moved from find-then-`item.delete()` to `ItemSearchOptions{service, account}.delete()`. `errSecItemNotFound` still maps to `NoEntry`, which our `delete` treats as success.
- **Windows: same target name.**
  - v3: `target_name: format!("{user}.{service}")`, i.e. `llm/<provider>/<uuid>.OpenAlpaca`, `CRED_TYPE_GENERIC`, `CRED_PERSIST_ENTERPRISE`, password as UTF-16LE (`REG/keyring-3.6.3/src/windows.rs:246, 378, 414-436`).
  - v4: `Store::new()` delimiters `["", ".", ""]` ("the configuration that matches … earlier versions of keyring", `REG/windows-native-keyring-store-1.1.0/src/store.rs:31-40`); target `"{d0}{user}{d1}{service}{d2}"`, which is the same string (`src/cred.rs:53-56`); persistence default `"Enterprise"` (`store.rs:121`); UTF-16LE decode (`src/utils.rs:275-297`).
  - **Not compiled or run on Windows here.** No Windows Rust target is installed and CI builds no Rust on Windows.
- **Linux / Secret Service: found, by a subset match.**
  - v3 creates items with attributes `{service, username, target: "default", application: "rust-keyring"}` in the default collection. It searches service-wide on `{target, service, username}`, then falls back to the default collection on `{service, username}` (`REG/keyring-3.6.3/src/secret_service.rs:243-248, 330-360, 392-420, 437-446`).
  - v4, for an entry created without a `target` modifier (ours), searches service-wide on `{service, username}` only (`REG/zbus-secret-service-keyring-store-1.0.1/src/cred.rs:102-110`; `src/service.rs:30-50`, `SearchItems` plus unlocking any locked matches). The Secret Service matches an item carrying *at least* the given attributes, so v3's items, which have extra `target` and `application` attributes, are found.
  - v4 creates new items in `"default"` with `{service, username}` (`cred.rs:118-124`). A rolled-back v3 build finds those through its legacy default-collection fallback.
  - The Secret Service matching semantics are my reading of the spec, not something I ran.
  - Not compiled for Linux here (no target installed). The CI ubuntu job is the first real build.

**Other behaviour changes [dep-source / our-code]:**
- v4 opens the platform store once per process, in a `LazyLock`, on the first `Entry::new` (`v1.rs:107-124`). On Linux that means connecting to D-Bus eagerly (`zbus…/src/store.rs:48-61`). If that fails (headless, no session bus), every `Entry::new` in the process returns `Error::NoDefaultStore`. v3 failed later, at `get_password`.
- Our code turns both into `Err(String)`. Consequences:
  - `CachingSecretStore::prefetch` → `false` → falls back to `MemorySecretStore` with the same WARN (`apps/openalpacad/src/services/llm.rs:36-49`);
  - the reverse migration logs and skips (`crates/openalpaca_llm/src/config/llm_config/migration.rs:178`, `:187`);
  - the CLI builds one `KeyringSecretStore` per command (`apps/openalpaca/src/commands/ai_config_helpers.rs:36-42`).

  So the user-visible outcome is unchanged; only the logged error text differs.
- **Exposure.** The keychain is opt-in: `[security] use_keychain` defaults to false (`apps/openalpacad/src/services/llm.rs:13-20`). With it off, keyring is touched only by the one-time reverse migration of leftover `secret_ref` keys (`:53-79`). I did **not** read the owner's `llm.toml` to see whether any `secret_ref` or `use_keychain = true` exists (`~/.openalpaca` is off limits). That is an open question.
- **macOS access prompts.** A prompt after upgrading comes from the new binary's identity, not from the keyring version. It is the same call path as today (the `CachingSecretStore` doc comment already notes prompts).
- **Transitive weight [measured, lock].** On Linux only, about 40 new crates: zbus 5.19, zvariant, async-io, async-executor, polling, blocking, num-*, secret-service 5.2 and others. On macOS: keyring-core and apple-native-keyring-store. Removed: dbus-secret-service.
- **MSRV.** keyring 4.2.0 declares 1.88 (`cargo +1.98.1 info keyring@4.2.0`, log `secrets-keyring-info.txt`). The lock was resolved MSRV-aware at 1.93, and the 1.93 check passed for llm, storage and core on macOS.

## blake3 1.8.7, rand 0.10.3

- **blake3.** Locked 1.8.7 is the latest (`SP/direct-deps.tsv`). Its lock entry is unchanged by this patch, and nothing moves.
- **rand 0.10.2 → 0.10.3.** A semver-compatible lock refresh (`SP/cargo-update-dryrun.txt`). The changelog lists fixes to `WeightedIndex`, float `Uniform::new_inclusive` and deserialized `Uniform<char>` ([docs] https://raw.githubusercontent.com/rust-random/rand/master/CHANGELOG.md, as summarized by the fetch tool).
- **Our rand use** [our-code]: `rng().random_range` (`openalpaca_llm/src/routing/rate_limiter/mod.rs:148`), `rng().random::<[u8; 32]>()` (`openalpaca_storage/src/discovery/mod.rs:77`), `Alphanumeric` sampling (`apps/openalpacad/src/routes/auth.rs:22`) and `routes/command.rs:153`. None touches the fixed APIs. Needs nothing (inferred, not measured). Leave it to the lock-refresh commit.

## Risks

**Persisted data:**
- The SHA-256 hex spellings (owner keys, partition keys, note revisions, `file_assets.sha256`) are measured identical. A future `{:x}` edit cannot silently diverge, because it will not compile.
- `.master_key` format and semantics are unchanged: 64 lowercase hex characters, no key derivation. Measured: shape and read-back, with keys minted by both versions cross-loaded.

**Secrets:**
- `secret_encrypted` is measured compatible in both directions, including rollback.
- OS keychain entries: established from source only, not measured. macOS and Windows use an identical identifier. On Linux the match relies on the Secret Service's subset matching.
- **Worst case if the keychain reading is wrong:** a `use_keychain = true` user sees their keys as missing (NoEntry), and the reverse migration logs "not found in keychain, skipping". The secrets stay in the OS store and are not deleted; the settings service deletes a keychain entry only on explicit key replace or remove (`crates/openalpaca_llm/src/config/settings_service.rs:374`, `:403`), and the reverse migration never deletes one (no `delete` call in `config/llm_config/migration.rs`). So the failure mode is "keys unavailable until rollback", not loss.

**Wire formats:**
- Persona ETags are identical (measured through the known-answer test and the core tests).
- Memory cursors and the discovery token (base64 URL_SAFE_NO_PAD) are identical (measured).
- Image `data:` payloads (base64 STANDARD) are identical (measured).

**User-visible:**
- base64 `InvalidLastSymbol` error wording (measured).
- keyring failure log wording on a headless Linux machine (source).
- Nothing else.

## Proposed commits (in order)

1. **`test(llm,storage): pin the on-disk secret and hash formats before the crypto bumps`**
   - Add to `crates/openalpaca_llm/src/keys/key_encryption/tests.rs` the fixture test `a_secret_written_by_the_aes_gcm_0_10_build_still_decrypts` (text in `SP/scratch-secrets/kat-test.rs.txt`; fixture above).
   - Add a known-answer test for `content_io::sha256_hex(b"abc")` (the `file_assets.sha256` spelling, which today has no literal test).
   - The memory-key and revision known-answer tests already exist.
   - *Verification:* `cargo test -p openalpaca_llm -p openalpaca_storage` on 1.93 at HEAD, then again after each later commit. Measured after the bump (run 8), not yet at HEAD in-tree; the fixture itself was produced by HEAD's code in the scratch program.
2. **`build(deps): base64 0.23`**
   - `Cargo.toml` line and lock only.
   - Independent and trivial; it could equally go into the lock-refresh wave.
   - *Verification:* `cargo check --workspace --all-targets --exclude openalpaca_gui`; `cargo test -p openalpaca_llm -p openalpaca_storage`; `cargo test -p openalpaca_core --lib -- update_persona memory`.
   - Optional: `default-features = false, features = ["std"]` (owner's call).
3. **`build(deps): sha2 0.11 (digest 0.11, hybrid-array)`**
   - `Cargo.toml`, lock, and the 10 `{:x}` rewrites in 7 files.
   - Independent of aes-gcm in code (disjoint files). The two share crypto-common 0.2 and hybrid-array in the lock.
   - *Verification:* as in 2, plus `cargo test -p openalpacad -- persona hot_reload`. I did **not** run the daemon tests; the daemon changes are in-memory hashes plus one ETag helper whose core-side known-answer test passed.
4. **`build(deps): aes-gcm 0.11 (aead 0.6)`**
   - `Cargo.toml`, lock and `key_encryption/{mod,tests}.rs`.
   - *Verification:* `cargo test -p openalpaca_llm`, including commit 1's fixture test; optionally re-run `SP/scratch-secrets/compat`.
   - Commits 3 and 4 can be squashed into one "RustCrypto hybrid-array" commit if the plan prefers.
5. **`build(deps): keyring 4.2 (v1 feature; Secret Service over zbus on Linux)`**
   - `Cargo.toml` and lock only.
   - Riskiest of the four, only because it cannot be measured here. Keep it separate so it can be reverted alone.
   - *Verification:*
     - `cargo check` and `cargo test --workspace --exclude openalpaca_gui` on the CI ubuntu job (the first real Linux build of zbus);
     - a manual cross-version read on a **disposable** macOS user account or VM (not the owner's login keychain): set an entry with the HEAD build, `use_keychain = true`, then start the new build and confirm the key resolves;
     - on Linux, the same test in a CI container with gnome-keyring unlocked.
     - Windows stays unverified.
6. **`ci/docs: the Rust job no longer needs libdbus-1-dev`** (depends on 5)
   - Drop `libdbus-1-dev` from `.github/workflows/ci.yml:21`, keep `pkg-config`, and fix the comment at `:19`.
   - Narrow `README.md:29` to the desktop app, which still needs dbus through tao.
   - *Verification:* a green ubuntu CI run without the package.

All four crates build on Rust 1.93 (run 9, macOS, llm/storage/core), so commits 2–5 do not depend on the toolchain bump. Because the lock was resolved MSRV-aware at `rust-version = 1.93`, re-run `cargo update -p keyring -p aes-gcm -p sha2 -p base64` once the toolchain group raises `rust-version`, to pick up any newer transitive versions.

## Open questions

- Does the owner's `llm.toml` contain any `secret_ref` keys, or `[security] use_keychain = true`? If not, the keyring bump touches no existing secret at all. I did not read `~/.openalpaca`.
- A measured v3 → v4 keychain round trip needs a disposable keychain environment (a macOS VM or throwaway user, or a Linux container with gnome-keyring). The rules forbid using the real keychain, so it is established from source only.
- Windows (`windows-native-keyring-store`) was neither compiled nor run. The Windows release scripts are the only Windows CI.
- Should base64 0.23 go in now, given it duplicates 0.22 in the graph until axum and reqwest move? Should its `simd-unsafe` default be disabled?
- Daemon tests (`cargo test -p openalpacad -- persona hot_reload`) were not run in this group. They are compile-checked only.
