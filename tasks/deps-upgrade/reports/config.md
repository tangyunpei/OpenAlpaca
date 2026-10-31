# Configuration formats: toml 0.9 → 1.1, serde_yaml → a successor, console/dialoguer

Group: **configuration formats**. Worktree `SP/wt-config` (detached at `5e5ac317`), target dir `SP/target-198`, scratch `SP/scratch-config/`.
`SP` = `/private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/4b1ee495-f087-45a9-9fa8-4325993552c8/scratchpad`.

Evidence tags: **[measured]** (I ran it; command and result given), **[our-code]** (file:line in the worktree, identical to the main checkout at HEAD), **[dep-source]** (a path under `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`), **[docs]** (changelog / advisory / release page URL).

## 0. Bottom line

| Package | From → To | Result | Recommendation |
|---|---|---|---|
| `toml` | 0.9.12 → **1.1.6** | 0 compile errors, 0 source edits; every shipped TOML file parses to the same tree and re-serialises byte-identically; only TOML-1.1 "time without seconds" values print differently | **upgrade-now** (commit 1) |
| `toml_edit` | 0.25.13 → **0.25.15** | semver-compatible, performance-only releases; all M3 / config_io tests pass | **upgrade-now** (same commit) |
| `serde_yaml` | 0.9.34+deprecated → **serde-saphyr 1.3.0** (with compat options) — *owner decision* | 0 differences on 17 shipped files + 58 inline test fixtures; differences remain only on exotic YAML-1.1 spellings, listed in §2.4 | **replace**, as its own commit (commit 2), after the owner picks the successor |
| `console` | 0.16.4 → 0.16.6 | lock refresh only (not in my patch) | leave to the lock-refresh group |
| `dialoguer` | 0.12.0 → 0.12.0 | already latest | nothing to do |

Patch: `SP/patches/config.diff` (toml + toml_edit + serde-saphyr, **compiles**, all measured suites green). Alternative for the owner: `SP/patches/config-alt-serde_norway.diff` (toml + toml_edit + the rename-only fork `serde_norway`; core lib compiles and its skill tests pass — the rest of the workspace was not re-run for this variant).

## 1. What I ran

All cargo commands were prefixed `nice -n 10 env CARGO_TERM_COLOR=never CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=…`, one at a time, output to `SP/logs/config-*.log`.

| Step | Command (abridged) | Result |
|---|---|---|
| Baseline probes (toml 0.9, serde_yaml) | temporary integration tests `tests/zz_config_probe.rs` in core, llm, plugins (sources kept in `SP/scratch-config/probes/`, deleted from the worktree before the patch was saved); `cargo +1.98.1 test -p openalpaca_{llm,plugins,core} --test zz_config_probe` with `PROBE_OUT=…/probe-out/baseline` | 28 artefacts written |
| Lock update | `cargo +1.98.1 update -p toml@1.1.4+spec-1.1.0 --precise 1.1.6+spec-1.1.0`, then manifest edits, then `cargo metadata` (resolves) | see §1.1 |
| Compile | `cargo +1.98.1 check --workspace --all-targets --exclude openalpaca_gui` | exit 0, **0 errors**, 1 warning (future-incompat `proc-macro-error2 v2.0.1`, reached through `teloxide → aquamarine`; pre-existing, also in `SP/logs/ci-audit-main.json`) — `config-check-final.log` |
| Compile on the pinned toolchain | `cargo +1.93.0 check -p openalpaca_core --all-targets`; `cargo +1.93.0 clippy -p openalpaca_core -p openalpaca_llm -p openalpaca_plugins --all-targets` | both exit 0; 0 clippy warnings in any file the patch touches (`grep -c middleware/skill` = 0) |
| Probes after the bump | same three probe tests, `PROBE_OUT=…/bumped`; `diff -r baseline bumped` | only the differences in §1.2 |
| Tests | `cargo +1.98.1 test -p openalpaca_llm` | **245 passed**, 0 failed |
| | `cargo +1.98.1 test -p openalpaca_plugins` | **117 passed**, 0 failed |
| | `cargo +1.98.1 test -p openalpaca_core` (final, with the new pin test) | **2024 passed**, 0 failed, 5 ignored; integration binaries 14 + 1 passed |
| | `cargo +1.98.1 test -p openalpaca config` (CLI, incl. `daemon_config_cli`) | **32 passed** |
| | `cargo +1.98.1 test -p openalpacad -- routes::skills routes::extensions hot_reload services::agents bootstrap` | **74 passed** (filtered to the config/skill/extension-config paths; the MCP hang tests were deliberately not selected) |
| Pin test fails without the compat options | temporarily replaced `from_str_with_options(yaml_str, yaml_options())` by `from_str(yaml_str)`, ran `cargo test -p openalpaca_core --lib middleware::skill`, restored the file | **FAILED** (duplicate-key refusal, first assertion) — `config-test-core-skill-defaults.log` |
| cargo-audit | `SP/tools/bin/cargo-audit audit --db SP/tools/advisory-db --no-fetch --stale --json` (DB commit `e2111519`, 2026-09-25) | same 5 vulnerabilities and same warnings as main; **no advisory for any crate this patch adds**; dependency count 732 → 736 |
| Scratch comparisons | `SP/scratch-config/yamlcmp` (serde_yaml 0.9.34, serde_yaml_ng 0.10.0, serde_norway 0.9.42, serde-saphyr 1.3.0 default and compat, serde_yaml_bw 2.5.8) and `SP/scratch-config/tomlcmp` (toml 0.9.12 and 1.1.6 side by side, renamed deps) | §2.4, §1.2 |

### 1.1 Lock changes [measured]

`git diff Cargo.lock`: **added** `serde-saphyr 1.3.0`, `granit-parser 1.3.0`, `annotate-snippets 0.12.16`, `arraydeque 0.5.1`, `encoding_rs_io 0.1.8`, `nohash-hasher 0.2.0`; **removed** `serde_yaml 0.9.34+deprecated`, `unsafe-libyaml 0.2.11`; **updated** `toml 1.1.4 → 1.1.6` (the entry tauri already used), `toml_edit 0.25.13 → 0.25.15`; our five crates' `toml` edge moved `0.9.12 → 1.1.6`.

Lock-ordering trap [measured]: a plain `cargo update -p toml@1.1.4` *after* the manifest edit moved **tauri-utils** from toml 1.1.4 **back to 0.9.12** (its requirement is `>=0.9, <=1`). Doing the `--precise 1.1.6` update first, then editing the manifests, keeps tauri-utils on 1.1.6. The committed lock must be produced in that order (or checked with `awk '/name = "tauri-utils"/,/^$/' Cargo.lock | grep toml`).

Per binary [measured, `cargo tree -p <bin> --target all -e normal,build`]:

- `openalpacad`, `openalpaca`: before — `toml 0.9.12`, `toml_datetime 0.7.5` **and** `1.1.1`, `winnow 0.7.15` **and** `1.0.4`, `serde_yaml`, `unsafe-libyaml`; after — one of each: `toml 1.1.6`, `toml_datetime 1.1.1`, `toml_parser 1.1.3`, `toml_writer 1.1.2`, `serde_spanned 1.1.1`, `winnow 1.0.4`, `toml_edit 0.25.15`, plus `serde-saphyr`/`granit-parser`. The bump **removes** a duplicated `toml_datetime` and `winnow` from both binaries.
- `openalpaca_gui` keeps `toml 0.8.2`, `0.9.12`, `1.1.6`, `toml_edit 0.19.15/0.20.2/0.25.15`, `toml_datetime 0.6.3/0.7.5/1.1.1`, `winnow 0.5.40/0.7.15/1.0.4` — none of it ours (§3.3).

### 1.2 Probe diff, baseline vs bumped [measured, `SP/scratch-config/probe-diff.txt`]

Byte-identical: `load_daemon_config` of `config/daemon.toml` and the release template (as JSON), `toml::to_string_pretty` of both loaded `DaemonConfig`s, `toml::to_string(DaemonConfig::default())`, `McpConfig::parse` of `config/mcp.toml`, the template and a rich stdio+http block **including every `config_fingerprint`**, `toml::to_string_pretty(AgentConfigFile)` and its round trip, `read_config` + `render_config` of the llm.toml template, `render_config_preserving` of a provider toggle + added encrypted key + priced model (the M3 edit path), `render_config` of the edited config, `PluginManifest::from_dir(tests/fixtures/echo-plugin)`, and the parsed frontmatter + body + sections of all four shipped skills.

Different:
1. `plugin-config.json` (`config_codec::toml_to_json`): `short_time = 07:32` → `"07:32:00"` before, `"07:32"` after; `short_dt = 1979-05-27T07:32Z` → `"…T07:32:00Z"` before, `"…T07:32Z"` after. Values *with* seconds are unchanged.
2. `render_skill_markdown` output: `version: 0.1.0` → `version: "0.1.0"`, single-quoted strings → double-quoted. The re-parse equals the original in all four cases. `render_skill_markdown` has **no production caller** (only `skill/tests.rs:105,132`) [our-code].

## 2. Findings per package

### 2.1 toml 0.9.12 → 1.1.6

**Breaking changes in 1.0 [docs: https://github.com/toml-rs/toml/blob/main/crates/toml/CHANGELOG.md, saved at `SP/scratch-config/toml-CHANGELOG.md`]**: `Time::second`/`Time::nanosecond` wrapped in `Option` (toml_datetime 1.0), deserializing into borrowed `&str` allowed. 1.0.2–1.0.4: stricter digit-separator errors (`-_1`, `0b_1`, `1__2`), "allow dotted keys to extend implicit tables", "error when a table extends a dotted table that extended an implicit table". 1.1.0: MSRV 1.85. 1.1.1: stack-overflow fixes for runs of `=`/`+`/`-`. 1.1.3: writer overflow with >255 consecutive quotes. 1.1.4: preserve `Value::Datetime` into other formats. 1.1.6: fewer allocations.

**Our call sites.** No code touches `Time`, `Spanned`, `Deserializer`, `toml::ser` or `toml_datetime` directly [our-code: `grep` for those names finds only `config_codec.rs:23`, which uses `Datetime`'s `Display`]. API used (occurrences, tests included): `from_str` (78), `Value` and its variants, `Table`, `map::Map`, `Value::try_from`, `Value::try_into`, `to_string` (6), `to_string_pretty` (5), `de::Error` (1). All compile unchanged on 1.1.6 [measured: 0 errors].

**Features [dep-source: `toml-0.9.12+spec-1.1.0/Cargo.toml` and `toml-1.1.6+spec-1.1.0/Cargo.toml`]**: identical feature tables; `default = [std, serde, parse, display]`; `preserve_order` only via `preserve_order` or `fast_hash`. Resolved features on `toml 1.1.6` for the whole workspace: `default, display, parse, serde, std` — **no `preserve_order`** [measured: `cargo tree --workspace -e features -i toml@1.1.6 --target all`], so `toml::Table` stays a sorted `BTreeMap` exactly as on 0.9 (the fingerprint code does not depend on it anyway — `fingerprint.rs:96-104` sorts its own keys).

**Files we parse with `toml`** [our-code]:

| File | Reader | What a parse failure does |
|---|---|---|
| `daemon.toml` | `daemon_config/mod.rs:140` (via `Value`, then `try_into`) | **falls back to `DaemonConfig::default()` with a WARN** (`:148-155`) — silently loses the owner's limits/cost caps |
| `daemon.toml` (CLI) | `apps/openalpaca/src/commands/daemon_config_cli/mod.rs:331, 388, 429` | command error |
| `llm.toml` | `openalpaca_llm/src/config/llm_config/mod.rs:62, :90`; `router_builder.rs:44` | error; **`write_config` (`mod.rs:102-106`) then writes a full render instead of an edit** |
| `mcp.toml` | `tools/mcp/config.rs:196` | load error |
| tool configs | `tools/config/mod.rs:65` | error |
| `plugin.toml` | `openalpaca_plugins/src/manifest.rs:170` | plugin not loaded |
| `.permissions.toml` | `permission_gate.rs:532` (`parse_permissions`, also the writer's post-write check) | error |
| `.config/<plugin>.toml` | `permission_gate.rs:395` (**empty map with a WARN on failure**), `:479` (post-write check) | plugin sees no config |
| legacy `config/agents/<id>.toml` | `apps/openalpacad/src/services/agents.rs:98`; `agent/config_service.rs:111` | agent not loaded |

**Files we write with `toml` rather than `toml_edit`** [our-code]:

| Writer | Output | Bump effect |
|---|---|---|
| `openalpaca config set|unset daemon.*` — `daemon_config_cli/mod.rs:408, :438` (`to_string_pretty(&toml::Value)`) | rewrites `daemon.toml` whole (pre-existing: drops every comment and sorts keys — unrelated to the bump, not fixed) | byte-identical output for both shipped daemon.toml files [measured, §2.2] |
| `render_config` — `llm_config/mod.rs:73-77` (`Value::try_from` + `to_string_pretty`) | a new or unparseable `llm.toml`; also the source of the replacement items in the M3 editor (`edit.rs:48`) | byte-identical [measured] |
| `AgentConfigService` — `agent/config_service.rs:50, :86` (`to_string_pretty(&AgentConfigFile)`) | `config/agents/<id>.toml` when no `.md` exists | byte-identical [measured] |
| `toml_value_to_item` — `permission_gate.rs:535-544` | one plugin config value bridged into `toml_edit` | plugins suite green [measured] |
| `config_fingerprint` — `fingerprint.rs:49`, scalar `Display` at `:128` | in-memory blake3 digest, held in the extension ledger (`extensions/ledger.rs:135`), not persisted | identical digests [measured] |
| `toml_to_json` — `config_codec.rs:23` | plugin config as JSON: sent to the plugin in `initialize` and served by `GET /v1/extensions/{kind}/{id}/config` | **differs for seconds-less times** (§2.2) |

`mcp.toml`, `.permissions.toml`, the plugin config store and `llm.toml` edits go through `toml_edit` (`config_io`, `atomic_write_toml`, `edit.rs`).

### 2.2 Would anything parse or serialise differently? [measured, `SP/scratch-config/tomlcmp-results.txt`]

`tomlcmp` parses each file with both versions into `Value`, compares the trees (floats by `{:?}`, datetimes by `Display`), both serialisers' bytes (`to_string`, `to_string_pretty`), every leaf's `Display`, and cross-reads each version's output with the other.

- **7 shipped TOML files** (`config/daemon.toml`, `config/mcp.toml`, `config/tools/example.toml`, the three release templates, `tests/fixtures/echo-plugin/plugin.toml`): equal trees, identical `to_string` and `to_string_pretty` bytes, identical leaf `Display`, clean cross-reads.
- **42 adversarial cases** (`corpus/adversarial/`): 40 identical outcomes (TOML 1.1 inline-table newlines and trailing commas, `\e`/`\xHH`, floats incl. `-0.0`/`nan`/`inf`/`5e-324`, datetimes with `t`/space/9+ fraction digits, 300 quotes, BOM, CRLF, arrays of tables, mixed arrays, quoted/empty keys, deep nesting refusal, overflow refusal, leading-zero refusal, `-_1`/`0b_1`/`1__2` refusal, duplicate keys, table redefinition, extending an inline table…). Two differ:
  1. **Times without seconds** (`t = 07:32`, `1979-05-27T07:32Z`, `1979-05-27 07:32` — TOML 1.1 syntax both versions accept): 0.9 normalises to `07:32:00`, 1.1 keeps `07:32` in the tree, in `to_string`/`to_string_pretty` and in `Display`. 0.9 reads 1.1's output back as the same time (a downgrade can read it). **Where it shows**: only `config_codec::toml_to_json` (plugin config JSON) and a CLI/`render_config` rewrite of such a value; none of `DaemonConfig`, `LlmRouterConfig`, `McpConfig`, `AgentConfigFile` or `PermissionEntry` has a datetime field (`approved_at` is a `String`, `permission_gate.rs:71`) [our-code].
  2. **Dotted keys extending an implicit table** (`[a.b.c]` then `[a]` `b.d = 2`): **0.9 refuses, 1.1 accepts** (the 1.0.3 fix). Lenient direction only: no file that loaded stops loading; a `daemon.toml` of that shape that 0.9 silently replaced by defaults would start taking effect.
- **Error text**: 15 of 17 shared error cases print byte-identical messages; the two 1.0.4 cases report a different line/column [measured, `toml-error-messages.txt`].
- **Struct serialisation** (a stand-in with the field shapes `LlmRouterConfig` uses — `Option`, `u32/u64/i64`, `f64`, `f32`, snake_case enums, `HashMap`/`BTreeMap`, `Vec` of tables, strings needing escapes): `to_string`, `to_string_pretty`, and `Value::try_from` → `to_string_pretty` identical; trees equal [measured, `tomlcmp-structs.txt`]. Why: toml 0.9.12 and 1.1.6 already share `toml_parser 1.1.3` and `toml_writer 1.1.2` in both locks [measured], so tokenising and writing were never different; what differs is toml's own table assembly and `toml_datetime` 0.7 → 1.1.

**Shared helpers**: `toml 1.1.6` and `toml_edit 0.25.15` both depend on `toml_datetime 1.1.1`, `toml_parser 1.1.3`, `toml_writer 1.1.2`, `winnow 1.0.4` (plus `serde_spanned 1.1.1` for toml) — **no duplication** in the daemon or CLI [measured, `Cargo.lock`]. These are the latest releases (`cargo update --dry-run -p toml_parser -p toml_writer -p toml_datetime@1.1.1 -p serde_spanned@1.1.1 -p winnow@1.0.4` locks 0 packages).

### 2.3 toml_edit 0.25.13 → 0.25.15

0.25.14 "reduced allocations when parsing/rendering; faster rendering", 0.25.15 "reduced allocations when rendering arrays of tables" — no behavioural entry [docs: https://github.com/toml-rs/toml/blob/main/crates/toml_edit/CHANGELOG.md]. The M3 editor output is byte-identical [measured, probe `llm-template.edit.toml`], and the llm (245), plugins (117) and core (`config_io`) suites pass [measured]. The requirement is raised to `"0.25.15"` in the patch; leaving `"0.25"` and refreshing the lock would be equivalent.

### 2.4 serde_yaml replacement

**Where YAML is parsed** [our-code]: only SKILL.md frontmatter — `middleware/skill/mod.rs` `parse_skill_frontmatter` / `parse_skill_markdown` (two `from_str` sites), reached from the catalog (`orchestrator/skill/catalog/mod.rs:343, :667, :828`); a file that fails is skipped with a WARN (`catalog/mod.rs:296-300`). The one serialiser, `render_skill_markdown` (`renderer.rs:13`), has only test callers. **Not YAML-crate parsed**: agent templates (hand scanner, `agent/template/mod.rs:216`), persona docs SOUL/USER/IDENTITY/BOOTSTRAP (hand grammar, `middleware/persona_frontmatter.rs:1-8` — "Not YAML, deliberately"), and **memory notes** (hand-written canon, `openalpaca_storage/src/memory_files/note.rs:6`, ruling Q2 at `tasks/memory-redesign/03-design.md:684` — "no YAML crate"). The storage crate has no YAML dependency. So the replacement touches no persisted memory data; it changes how SKILL.md files — shipped, user-written, or written by the `create-skill` scripts (`config/skills/create-skill/scripts/write_skill_md.sh:113-200`, which emit unquoted `name: $NAME`, `net: $NET`, and raw `scripts_yaml`) — are read.

Typeless positions matter most: `scripts[].parameters: serde_json::Value` (`types.rs:237`) becomes the tool's JSON schema sent to the model (`to_tool_definition`, `types.rs:247-251`) — a **wire format**; `tools.defaults: HashMap<String, serde_json::Value>` (`types.rs:221`) is parsed but has no runtime effect.

**Candidates** [measured via crates.io API, GitHub API and advisory-db on 2026-09-25; code comparisons are dep-source]:

| Crate | Latest (date) | Repo | Implementation | RustSec | Measured vs serde_yaml 0.9.34 (158 files: value / typed) |
|---|---|---|---|---|---|
| serde_yaml | 0.9.34+deprecated (2024-03-25) | dtolnay/serde-yaml **archived** | unsafe-libyaml 0.2.11 | only RUSTSEC-2018-0005 (patched ≥0.8.4); **no unmaintained advisory**; cargo-audit on main reports nothing for it | baseline |
| serde_yaml_ng | 0.10.0 (2024-05-26) | acatton, last push 2025-09-14 | **same code** as serde_yaml after renaming (42 non-comment diff lines, clippy-style; `SingletonMap` made public) and the **same unsafe-libyaml 0.2.11** | none | **0 / 0** |
| serde_norway | 0.9.42 (2024-12-21) | cafkafk, last push 2025-08-04 | same code after renaming (40 non-comment diff lines); unsafe-libyaml-norway 0.2.15 = unsafe-libyaml + 10 clippy lines | none; RUSTSEC-2025-0068 lists it as "maintained fork" | **0 / 0** |
| serde_yml | 0.0.13 (2026-05-27) | **archived** | libyml | **RUSTSEC-2025-0068** (unsound, unmaintained); libyml **RUSTSEC-2025-0067** | excluded |
| serde-saphyr | **1.3.0 (2026-09-16)**; 1.0.0 on 2026-07-31, releases every ~2 weeks | bourumir-wyngs, pushed 2026-09-25, 2 open issues | pure Rust on granit-parser (a saphyr-parser fork), `unsafe` forbidden, fuzzed, YAML test suite | none | defaults **32 / 20**; with the compat options **15 / 13**, none on real files |
| serde_yaml_bw | 2.5.8 (2026-09-07) | same author | granit-parser **and** unsafe-libyaml-norway | none | 3 / 6 |

Semantics [measured, `SP/scratch-config/yamlcmp-diff-table.txt`]: **17 shipped files** (4 skills, 9 agent templates, 4 persona templates) and **58 skill-shaped inline fixtures extracted from the Rust tests**: **identical under every candidate**, default or compat. All differences come from the 83 hand-written YAML-1.1-isms:

- serde-saphyr **defaults** (why they are not acceptable here): into `serde_json::Value`, `y`/`n`/`yes`/`no`/`on`/`off` — **including a bare `N`** (`name: N` → `false`) — become booleans, so a `parameters` schema `enum: [yes, no]` or `default: yes` would reach the model as `true/false`; `<<` merge keys are applied (`invoke: {<<: *d}` changes `invoke.mode`); duplicate map keys are refused (a skill with one stops loading); `.inf`/`.nan` in typeless positions are refused; `!!binary` is decoded; errors quote the file (multi-line snippet).
- serde-saphyr **with the compat options in the patch** (`strict_booleans: true`, `merge_keys: AsOrdinary`, `duplicate_keys: LastWins`, `reject_non_finite_typeless_float: false`, `ignore_binary_tag_for_string: true`, `with_snippet: false`) still differs on:
  - typeless `010` → `10.0` (serde_yaml: string `"010"`), `1_000` → `1000` (serde_yaml: string) — e.g. a `minimum: 010` in a `parameters` schema;
  - typeless `.inf`/`.nan` → strings `".inf"`/`".nan"` (serde_yaml: `null`);
  - accepts what serde_yaml refused: quoted `"true"`/`"3"`/`1_0`/`!!int "3"` into typed bool/int fields, a duplicated **struct** field (last wins), u64-overflowing integers, surrogate-pair `\u` escapes, U+2028 in a plain scalar, a custom tag, a 100 000-node alias expansion (serde_yaml: "repetition limit exceeded");
  - refuses what serde_yaml accepted: a `~` (null) **map key** in `tools.defaults`; nesting deeper than 64 (serde_yaml's limit is 128 — dep-source `de/budget.rs` `max_depth: 64`);
  - `name:` (empty) is an error instead of `""` — both end as "skill not loaded";
  - error text changes wording (one line, e.g. `tabs disallowed within this context (block indentation) at line 4, column 2`) [measured, `yaml-error-messages-nosnippet.txt`].
- serde_yaml_bw: accepts `yes`/`on`/`y` into bool fields, refuses duplicate keys, decodes `!!binary` to bytes.
- Property interpolation (`${VAR}`) is compiled only with the non-default `properties` feature and `property_map` defaults to `None` [dep-source: `serde-saphyr-1.3.0/src/lib.rs:17`, `de/options.rs:527`].

**Recommendation (owner decision).** Three honest options:

- **A. Keep serde_yaml 0.9.34** — zero change; no advisory today; code frozen (archived); the `+deprecated` label stays.
- **B. `serde_norway` (or `serde_yaml_ng`)** — a rename; measured behaviour identical on all 158 files; but the code is the archived serde_yaml's, and neither fork has released for 21+ months (ng still ships the unmaintained unsafe-libyaml). It removes the label, not the dormancy. Patch: `SP/patches/config-alt-serde_norway.diff`.
- **C. serde-saphyr 1.3 with compat options** (the patch) — the only actively maintained option, safe Rust, 6 new small crates, MSRV 1.89 (≤ 1.93); identical on every real file measured; residual differences are the exotic spellings above, mostly in the lenient direction, and one new refusal class (null map keys, depth > 64).

I recommend **C** for the bump, as its own commit with the pin test, because it is the only choice that actually moves off unmaintained code; if the owner wants zero behaviour risk in this wave, **A** is equivalent to B in substance and costs nothing.

### 2.5 console / dialoguer — "appear twice"

[measured] `Cargo.lock` holds **one** `console` (0.16.4) and **one** `dialoguer` (0.12.0). `cargo tree` prints `console v0.16.4` twice because `openalpaca` depends on it directly (`apps/openalpaca/Cargo.toml:25`) **and** through `dialoguer 0.12.0` (`console ^0.16.0`); the second is the deduplicated `(*)` line — same version, one build. `cargo tree --workspace -d --target all` lists neither. The only other `console` requirement in the metadata is `similar 3.2.0 → console ^0.15.0` as a **dev-dependency** of `similar`, never resolved into our lock (`SP/meta.json`). Updates: `console 0.16.6` (semver-compatible lock refresh), `dialoguer 0.12.0` is latest.

## 3. Risks

### 3.1 Persisted data and secrets
- **llm.toml (holds encrypted keys)**: if the new parser refused a file the old one read, `write_config` would replace the owner's file with a full render (`llm_config/mod.rs:102-106`) and lose comments/unknown keys. Measured: the template and a keyed, priced config parse and re-render byte-identically, and the only parse-behaviour change is in the lenient direction. **Not measured: the owner's real `~/.openalpaca/config/*.toml`** (not read, by rule). Before landing, run the tomlcmp tool read-only on those files and compare only the equality bit — never print them (`SP/scratch-config/target/release/tomlcmp <file>` prints values; a no-print variant is a 10-line change).
- **daemon.toml**: a refusal silently means defaults (`daemon_config/mod.rs:148-155`) — cost caps, limits. Same measurement, same caveat.
- **Plugin config** (may hold secret *references*): parse failure reads as empty (`permission_gate.rs:395`). Same caveat.
- **Memory notes**: untouched (no YAML crate in `openalpaca_storage`).
- **SKILL.md**: read-only change; nothing re-serialises a skill in production.

### 3.2 Wire formats and what users see
- Plugin config JSON: seconds-less TOML times now keep their spelling (`"07:32"`), both to the plugin's `initialize` and on the config GET route.
- Skill `parameters` schemas: identical for every real file; with serde-saphyr, typeless `010`/`1_000`/`.inf` spellings change (§2.4).
- Skill load errors in the WARN log change wording (single line kept by `with_snippet: false`).
- `render_skill_markdown`'s quoting changes (test-only caller).
- MCP fingerprints: unchanged; they are not persisted anyway.

### 3.3 Out of our hands
`toml 0.8.2`/`toml_edit 0.20.2`/`toml_datetime 0.6.3` come from `system-deps 6.2.2` (the gtk-rs 0.18 `*-sys` build scripts); `toml_edit 0.19.15` from `proc-macro-crate 1.3.1` (`gtk3-macros`); `toml_edit 0.20.2` also from `proc-macro-crate 2.0.2` (`glib-macros`); `toml 0.9.12` from `cargo_toml 0.22.3` ← `tauri-build 2.6.3` (latest stable; `cargo_toml 1.0.1` exists, tauri-build 2.x does not use it). All GUI, Linux-GTK or build-time only. `cargo_metadata 0.19.2` (via tauri-utils) has no toml dependency.

## 4. Proposed commits (in order; independent of the toolchain bump — both compile on 1.93.0)

1. **`chore(deps): toml 0.9 → 1.1 and toml_edit 0.25.15`** — `Cargo.toml` (`toml = "1.1"`, `toml_edit = "0.25.15"`), `Cargo.lock` produced as in §1.1 (precise-update toml 1.1.4 → 1.1.6 first, so tauri-utils stays on 1.x). No source change. *Verify*: `cargo check --workspace --all-targets --exclude openalpaca_gui`; `cargo test -p openalpaca_llm -p openalpaca_plugins -p openalpaca_core`; CLI `config` tests; the tauri-utils edge check; ideally the read-only equality check on the owner's real config files.
2. **`chore(deps): replace the archived serde_yaml with serde-saphyr`** (after the owner's decision; option B is a 5-line alternative) — `Cargo.toml`, `crates/openalpaca_core/Cargo.toml`, `middleware/skill/mod.rs` (`yaml_options` + `parse_yaml`), `renderer.rs`, the pin test `yaml_1_1_spellings_read_the_way_serde_yaml_0_9_read_them` (`skill/tests.rs:326`), `Cargo.lock`. *Verify*: the pin test passes and **fails with the options removed** (shown); its assertions hold under serde_yaml 0.9.34 (`SP/scratch-config/pin-serde_yaml.txt`); `cargo test -p openalpaca_core`; daemon `routes::skills`; re-run `yamlcmp` on any skill directory the owner has (read-only).
3. (Docs group) The memory-redesign records still describe a `serde_yaml` edge into storage (`tasks/memory-redesign/03-design.md:213, :723`, `04-implementation-plan.md:77, :146`) that ruling Q2 replaced; a CLAUDE.md line on the YAML crate choice if C is taken.

## 5. Files

- Patch: `SP/patches/config.diff` (compiles; 6 files, +148/−48). Alternative: `SP/patches/config-alt-serde_norway.diff`.
- Probes: `SP/scratch-config/probes/{core,llm,plugins}_probe.rs`; outputs `SP/scratch-config/probe-out/{baseline,bumped}/`; diff `SP/scratch-config/probe-diff.txt`.
- YAML: `SP/scratch-config/yamlcmp/` (program, corpus of 17 shipped + 83 adversarial + 58 extracted fixtures), `yamlcmp-results.txt`, `yamlcmp-diff-table.txt`, `yaml-error-messages*.txt`, `pin-serde_yaml.txt`.
- TOML: `SP/scratch-config/tomlcmp/` (program + corpus), `tomlcmp-results.txt`, `tomlcmp-structs.txt`, `toml-error-messages.txt`; changelogs `SP/scratch-config/*-CHANGELOG.md`.
- Logs: `SP/logs/config-*.log`, `config-audit-after.json`, `config-tree-*.txt`.
