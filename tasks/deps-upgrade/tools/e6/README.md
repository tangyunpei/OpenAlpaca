# E6 — the read-only comparators for the owner's real config and skill files

The plan of record is now [`../../programme-plan.md`](../../programme-plan.md) (since 2026-10-20); the plan-rev3 sections cited below are frozen history and still resolve.

**Status: written 2026-10-11 (`df477c67`, plan-rev3 §6 E6); amended 2026-10-17 for the plan review (`tasks/deps-upgrade/reports/2026-10-17-plan-review.md`: F-13, SF-14, A2-16, MF-5 and F-08) and the ZCode audit (`tasks/research/zcode-audit-2026-10-17.md`: Z3-05, A-07); re-pinned 2026-10-20 to the cut's versions before the consent point (`tomlcmp` on toml 1.1.8, `yamlcmp` on serde-saphyr 2.0.0 under owner decision D4 (a), with C1's `emit_comments: false`); on 2026-10-27, `consent-point.sh` and its test — the consent point as one owner-run command, the owner's option (a) — *One command* and *Revision log*, below. It moves no version and touches no product code; it is run by the owner, at plan-rev3 §5.1 item 3's consent point.**

The dependency wave moves the three crates that read the owner's configuration:
`toml` 0.9.12 → 1.1.8 (P29; *Re-pin* below), `toml_edit` 0.25.13 → 0.25.17 (P04's lock
refresh) and `serde_yaml` 0.9.34 → `serde-saphyr` 2.0.0 (owner decision D4 (a)) with
serde_yaml-compatible options (P30). Plan-rev3 §6 E6 asks, **before P04, P12, P29 and P30**, whether
the owner's real files read the same under both versions — with tools that print
only an index and an equality bit, never a value, a key or a path, and that
cannot write. It runs at plan-rev3 §5.1 item 3's consent point, with the DU-10
count beside it (review A2-16, SF-14).

These five crates are those tools: four rebuilt from the September ones (R13: the
archived `tedit-0.25.15` carried a diagnostic its 0.25.13 twin lacked, and the
archived `yamlcmp` wrote `render-*.yaml` beside its corpus and walked every
`.md`), and `keycount`, the DU-10 count, new on 2026-10-17 (review SF-14). Each
is one source; the old/new pair of `tedit` is one `src/main.rs`
under two manifests that differ in the `toml_edit` line alone (`diff
tedit/old/Cargo.toml tedit/new/Cargo.toml`).

| Tool | Compares | Loader it mirrors |
|---|---|---|
| `tomlcmp` | toml 0.9.12 vs 1.1.8, the toml P29 takes at the cut (*Re-pin* below), in one binary | `toml::from_str::<Value>`, `to_string`, `to_string_pretty` (`render_config`), the leaf `Display` (`config_codec::toml_to_json`) |
| `tedit` (`old/`, `new/`) | toml_edit 0.25.13 vs 0.25.17, two binaries | `render_config_preserving`'s `apply_delta` (copied verbatim from `crates/openalpaca_llm/src/config/llm_config/edit.rs:74-184`), with the September mutation set (six seeds, `s * 7919 + 17`) |
| `yamlcmp` | serde_yaml 0.9.34 vs serde-saphyr 2.0.0 (D4 (a)) + P30's options, `emit_comments: false` among them (C1), in one binary | `parse_skill_frontmatter` (`middleware/skill/mod.rs:51-56`): the frontmatter into `SkillFrontmatter` (a copy of `middleware/skill/types.rs`; `consent-point.sh` derives it from `--base` instead, *One command*), validated |
| `typedprobe` | nothing — it runs the loaders of the tree it is built against, by path; built twice, from the branch point and from the prep tree (review F-08) | `load_daemon_config`, the M3 `llm.toml` edit, `McpConfig::load`, `AgentConfigFile`, `PluginManifest::from_dir_for_load`, `parse_skill_markdown`, `parse_agent_markdown`, the four persona parsers |
| `keycount` | nothing — it counts, per file, the keys named `secret_ref` at any depth and the `use_keychain` keys set to `true` (DU-10) | `toml::from_str::<toml::Table>` with toml 0.9.12, the version the daemon reads `llm.toml` and the plugin files with until P29 (its `Cargo.lock` is the checkout's, trimmed, so the parser is the daemon's to the byte) |

Every tool takes an explicit list of files on its command line (nothing is
walked), prints one line per input — `<index>\t<word>`, the index the input's
1-based position in the argument list — and exits 0. `--summary` prints counts
only. The words:

- `equal` / `different` — both versions read the file; the result is the same / not.
- `parse-error-old` / `parse-error-new` / `parse-error-both` — one version or both refused the file.
- `panic-old` / `panic-new` / `panic-both` — one version or both panicked reading the file (review F-13; *Proving the panic path* below). A panic is reported before a parse error and counted as a panic only.
- `unreadable` — the file could not be read as UTF-8 text; `no-frontmatter` (yamlcmp) — no `---` block.
- typedprobe prints `ok`, `err` or `panic` instead: whether the real loader accepted the file (for `daemon` and `llm-edit` with extra conditions — see the source's header).
- keycount prints `<index>\tcounted\t<secret_ref>\t<use_keychain-true>`, or `parse-error`, `unreadable` or `panic` followed by `-\t-` — no count for that file. A key is counted whatever its value, in any spelling (an array of tables, `key = { secret_ref = … }`, a dotted key), so the count errs high, never low.

Keep your argument list in a file so that an index can be looked up on your
side; the tools never print the path.

## Build (once, into a scratch directory of your choosing)

```sh
R=$(git rev-parse --show-toplevel)     # the checkout at the branch point
E=$R/tasks/deps-upgrade/tools/e6
X=/path/to/a/scratch/dir               # outside the repository; removed whole when done
T=$X/target; B=$X/bin; mkdir -p "$B"
# The toolchain a tree pins, read from its rust-toolchain.toml (A2-16): never a
# literal, so the same lines build on a tree after P03.
tc() { sed -n 's/^channel *= *"\(.*\)"$/\1/p' "$1/rust-toolchain.toml"; }
# The build dir pinned to the target dir (MF-5): harmless before M63; from M63
# on, CARGO_TARGET_DIR alone would leave the intermediates under the build-dir
# in $CARGO_HOME, where removing $X never reaches them.
c() { local tc=$1 m=$2; shift 2
      CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="$T" CARGO_BUILD_BUILD_DIR="$T" \
        cargo "+$tc" build --manifest-path "$m" "$@"; }
cd "$R"
TC=$(tc "$R")
c "$TC" "$E/tomlcmp/Cargo.toml"   --locked && cp "$T/debug/tomlcmp"  "$B/"
c "$TC" "$E/yamlcmp/Cargo.toml"   --locked && cp "$T/debug/yamlcmp"  "$B/"
c "$TC" "$E/keycount/Cargo.toml"  --locked && cp "$T/debug/keycount" "$B/"
c "$TC" "$E/tedit/old/Cargo.toml" --locked && cp "$T/debug/tedit" "$B/tedit-0.25.13"
c "$TC" "$E/tedit/new/Cargo.toml" --locked && cp "$T/debug/tedit" "$B/tedit-0.25.17"

# typedprobe, built twice (F-08): against the branch point (this checkout:
# toml 0.9, its toolchain) and against the prep tree (the wave's manifests and
# lock: toml 1.1, Rust 1.99.x), each from a scratch copy beside a copy of that
# tree's lock, its three path lines and its `toml` line taken from that tree —
# so nothing is written inside either tree, even by a build cut short.
W=$X/prep-tree; rm -rf "$W"; mkdir -p "$W"
git -C "$R" archive refs/prep/deps-wave-2026-10-12 | tar -x -C "$W"   # the prep tree as it stands at the consent point
pkgs() { awk '/^name = /{n=$3} /^version = /{print n, $3}' "$1" | tr -d '"' | sort -u; }
tp() { local w=$1 name=$2 P=$X/typedprobe-$2; rm -rf "$P"; mkdir -p "$P"
       cp -R "$E/typedprobe/src" "$P/" && cp "$w/Cargo.lock" "$P/Cargo.lock"
       sed -e "s|\.\./\.\./\.\./\.\./\.\./crates/|$w/crates/|" \
           -e "s|^toml = .*|$(grep -m1 -E '^toml = ' "$w/Cargo.toml")|" "$E/typedprobe/Cargo.toml" > "$P/Cargo.toml"
       c "$(tc "$w")" "$P/Cargo.toml" && cp "$T/debug/typedprobe" "$B/typedprobe-$name"
       comm -23 <(pkgs "$P/Cargo.lock") <(pkgs "$w/Cargo.lock"); }   # prints `typedprobe 0.1.0` and nothing else
tp "$R" base
tp "$W" wave
```

The three comparators and `keycount` carry their own `Cargo.lock` (the exact
`toml_parser`, `winnow`, `granit-parser` the comparison ran on; `keycount`'s is
the checkout's lock trimmed to its one dependency), so they build `--locked`.
`typedprobe` takes the lock of the tree it is built against, because the
question it answers is about that tree's own graph; cargo trims the copy to
typedprobe's dependencies, which `--locked` would refuse, so `comm` checks
instead that no version moved (its only line is typedprobe's own). The prep
tree's ref is the one pinned today (tip `7e702582`, whose lock holds toml
1.1.8); at the consent point, export whichever commit carries the wave's
manifests and the cut's lock. Its build reads crates the checkout's graph does
not hold, so it needs the registry (or a cache that has them). `tc` assumes the
`channel = "…"` spelling both trees use today. Building both into one `$T`
under two toolchains is fine in a scratch directory; the one-toolchain rule is
about the repository's `target/`.

**Re-pin: the pins are the cut's answers.** The comparators were first pinned
to the 7-day rule's answers on 2026-10-11 (`toml11 = "=1.1.6"`, `toml_edit =
"=0.25.17"`, `serde-saphyr = "=1.3.0"`). On 2026-10-20, before the consent
point, they were re-pinned to what the cut (D1's instant, 2026-10-21T19:53Z)
takes, so that each comparison is against the version the daemon will be built
with: `toml11 = "=1.1.8"` (`tomlcmp/Cargo.toml`), the toml P29 takes, eligible
from 2026-10-16, which the prep tip's lock `7e702582` and the measured final
lock `../gbase/locks/recut-lock-99-final.lock` hold as `1.1.8+spec-1.1.0`;
`toml_edit = "=0.25.17"` (`tedit/new/Cargo.toml`), unchanged, already the
toml_edit P04 takes; and `serde-saphyr = "=2.0.0"` (`yamlcmp/Cargo.toml`), the
major owner decision D4 (a) takes, with `non_finite_float_policy: AsString` in
place of the deprecated `reject_non_finite_typeless_float` and `emit_comments:
false` in `yaml_options()` (C1 of `../../reports/2026-10-17-cut-compatibility.md`).
Each `Cargo.lock` followed its pin in one build without `--locked` — tomlcmp's
moved toml 1.1.6 → 1.1.8 and nothing else, yamlcmp's serde-saphyr and
granit-parser 1.3.0 → 2.0.0 and nothing else — so the Build block's `--locked`
builds are the ones to run. If the G-base run on the day prints another version
for any of the three, the same rule holds: that one line takes what the lock
holds, the tool is rebuilt once without `--locked` so its `Cargo.lock` follows,
and that build is the one run; a run on any other version answers nothing about
the wave.

## One command (the consent point, option (a))

The owner chose on 2026-10-27 to run the consent point now (option (a); programme-plan §5 item 3). `consent-point.sh` is the Build block, the owner-side run, the DU-10 count and the nothing-written proof below as one command. Quit the app and the daemon, then, from the checkout's root:

```sh
/bin/bash tasks/deps-upgrade/tools/e6/consent-point.sh --home ~/.openalpaca --include-checkout-config
```

It builds every tool from `git archive` exports of `--base` (default `HEAD`, never the working tree) and `--wave` (default `refs/prep/deps-wave-2026-10-12`) into a fresh `mktemp -d` directory under `$TMPDIR`. It builds the input lists the way the daemon loads its files: a skill is `skills/<child>/SKILL.md`, one level, as `scan_directory` reads it. This recipe's `find` takes any depth; a SKILL.md the scan never loads, and one under a plugin directory, is counted and not fed. It builds yamlcmp on the base's own `SkillFrontmatter` — `middleware/skill/types.rs` of the `--base` export with the committed copy's two documented changes, each anchor matched exactly once — not on the committed copy `yamlcmp/src/types.rs`, which is taken by hand and can fall behind (it lacks N-SKILL-2's `#[serde(flatten)] unknown_keys`, `8c63804f`, and the by-hand Build block above still builds it). Then it runs every comparator and probe and the DU-10 count, with a limit on each tool run (`--tool-time-limit`, default 300 s). With a warm cargo cache this takes a minute or two. The toolchain check runs `rustup toolchain list` from `/` with `RUSTUP_AUTO_INSTALL=0` and no `RUSTUP_TOOLCHAIN`: rustup 1.29 installs the active toolchain on that command when it is missing, and the checkout's working tree may pin one that is. `--include-checkout-config` also reads the checkout's ignored `config/llm.toml` and every other file of a loaded kind in `config/` whose bytes the export does not hold. Without the flag those files are counted and never opened, and P12's waiver cannot hold while that `llm.toml` exists. The script's header lists every option and step.

**What the block holds.** Paste everything between `=== PASTE FROM HERE ===` and `=== TO HERE ===`. The block holds:
- the two commits and their toolchains, and each build's crate versions;
- `base-matches-old-pins`: a base past P04 fails the run as `base-past-old-pins`;
- `old-side-stack-matches-base <tool> ...` for tomlcmp, tedit-old and keycount (informational): whether the TOML crates under the old sides (toml_parser, toml_writer, toml_datetime, serde_spanned, indexmap, winnow) are the base's. keycount's are; tomlcmp's and tedit-old's are the cut's toml_parser 1.1.5, toml_writer 1.1.3, toml_datetime 1.1.2, serde_spanned 1.1.2 and indexmap 2.14.2 against today's 1.1.3, 1.1.2, 1.1.1, 1.1.1 and 2.14.1. Their sources were compared on 2026-10-27 and differ in no behaviour: toml_parser's lexer sizes its token buffer differently and drops a call to the no-op `decode_whitespace`; toml_writer, toml_datetime and serde_spanned have identical sources; indexmap changes only its macros, which toml_edit does not use;
- `yamlcmp-struct source=base committed-copy-matches-base yes|no`: yamlcmp read with the base's struct; `no` says only that the committed copy is stale (informational);
- `wave-loader-behind-base files=<n> skill=... persona=... agent=... daemon-config=... mcp-config=... llm-config=... plugin-manifest=...`: which probed loaders `--base` changed since it and `--wave` parted (a file under the loader's paths, tests aside, changed between their merge base and `--base`), or `unknown`;
- per list, the number of inputs by source, `missing` and `skipped`;
- each tool's `--summary` counts, and every row whose word is not the all-clear, as `<tool> <index> <word>`;
- every index where the two typedprobe builds disagree, as `tp-<kind> <index> base=<word> wave=<word>`;
- keycount's rows and summaries and the line `P12-keychain-waiver holds|does-not-hold (<reason>)`;
- `store-unchanged` (every entry under `config/` and `plugins/`, with its type, mode, mtime, ctime, flags, inode and hash, taken before the exports and after the runs, so the builds' window is in it) and `inputs-unchanged` (every file fed to a tool, read through links, wherever it lives, the same fields and its size, taken once the lists exist, before the builds, and after the runs). Every change to a file's bytes or metadata moves its ctime — an extended attribute such as a Finder tag, a flag, an ACL — and reading it never does;
- any `step-failed <step>`, a status and the exit code.

It never holds a file name, a path, a key, a value or a byte of a file. A last filter drops any line with a byte outside `[A-Za-z0-9 ._:=(),+-]`, under `LC_ALL=C`.

**Reading a typedprobe disagreement.** The default `--wave` (`7e702582`) is the merge base `d7c6bbc6` plus the prep commits, 68 commits behind `HEAD` on 2026-10-27, so typedprobe-wave runs the wave's crates *and* the wave's older copy of every loader `--base` has changed since: `wave-loader-behind-base` names those kinds. A `tp-<kind> <i> base=<word> wave=<word>` row is the crate bump's alone only for a kind that line marks `no`; for a kind marked `yes` it can be the loader's own change.

**The YAML axis.** No ref carries both of the cut's YAML and TOML crates: the default `--wave` has the cut's toml 1.1.8 and toml_edit 0.25.17 but serde-saphyr 1.3.0, not 2.0.0. The block shows this as `wave-matches-new-pins ... serde-saphyr=no`. On that axis yamlcmp's typed row is the word of record: the base's struct on serde_yaml 0.9.34 (today's loader) against the same struct on serde-saphyr 2.0.0 with P30's options and C1 (P30's loader). Read every `tp-skill <i>` row against yamlcmp's row for `<i>`. `base=err wave=ok` beside a yamlcmp `parse-error-both` (a duplicated top-level key) is the wave's pre-N-SKILL-2 struct, not the bump: P30 refuses that file too. Beside a `parse-error-old` (a custom tag on an unknown key), today's loader refuses the file and P30 would load it. `base=ok wave=err` beside a yamlcmp `equal` is the 1.3.0 build's (for example 33 or more comment lines before a block value; *What to expect*).

**Where the index lists stay.** Stdout holds the block and nothing else, so `| pbcopy` or `> block.txt` takes exactly what is to be pasted. The one line after it is on stderr: it names the scratch directory and is not for pasting. Its `lists/<list>.list` maps each index to `<source>\t<path>`. It also holds the tools' outputs, the logs, and snapshots with every file path and the SHA-256 of every file the run read. The build tree (about 3.5 GB) is removed when the run ends unless `--keep` is given. Remove the rest when done. Exit 0 means the run completed, and a difference is a result. Exit 1 means a step could not run, 2 is a usage error, and 130, 143 and 129 mean Ctrl-C, TERM and hangup; in each of those cases the block is still printed.

**The test.** `TMPDIR=<a scratch dir> /bin/bash tasks/deps-upgrade/tools/e6/consent-point-test.sh` runs the script on synthetic stores only, with sentinels planted in keys, values and names; it takes a few minutes. Four runs that build the tools check the following:
- a fixture home under a UTF-8 locale, with `CDPATH` and a relative `--home`; its two struct-dependent skills read `parse-error-both` and `parse-error-old` on yamlcmp, which shows the base's struct is the one in use;
- an empty home, with every child that could reach the terminal shimmed, plus a rustc wrapper and `MAKEFLAGS`;
- pathological inputs under a 10 s limit; during the builds an extended attribute is set on a store file and on a fed file outside the store, a change to metadata only, and both lines must read `no`;
- Ctrl-C during the builds.

Runs that stop at the pre-flight or build nothing cover the rest:
- the usage refusals;
- a closed stdout or stderr, and a scratch root mktemp cannot write;
- a scratch root inside `--home`'s `config` or `plugins` through a link, and `--home /`;
- an inherited xtrace;
- a missing toolchain;
- a working tree pinning a toolchain that is not installed: a rustup shim runs the real rustup only for a call from `/` with `RUSTUP_AUTO_INSTALL=0`, so the test cannot install anything;
- the alphabet filter under `en_US.UTF-8`;
- the opt-in sources in a throwaway checkout.

Every check requires that no sentinel and no fixture path reach either stream, that stdout end at `=== TO HERE ===`, and that stderr hold only the scratch-dir line.

## The owner-side run (plan-rev3 §5.1 item 3, §6 E6)

**The files.** `llm.toml`, `daemon.toml`, `mcp.toml` in `~/.openalpaca/config/`
and in the checkout's `config/`. The checkout tracks no `llm.toml` (the seed is
`scripts/release/templates/config/llm.toml`), but a daemon started from the
checkout finds `./config` by walking up from its executable or its working
directory, not through `OPENALPACA_CONFIG_DIR`
(`apps/openalpacad/src/bootstrap/config.rs:14`), and seeds an ignored
`config/llm.toml` there (`:84`); include that copy when it is present (`LLM`
below). Then any other `OPENALPACA_CONFIG_DIR`;
`~/.openalpaca/plugins/.permissions.toml`, `plugins/.config/*.toml`, each
`plugins/*/plugin.toml`; `config/tools/*.toml`; any legacy `config/agents/*.toml`;
every `SKILL.md` under the skill directories, plugin-contributed ones included
(`find <dir> -name SKILL.md`, then pass the list — the tool itself walks nothing).

Write each list to a file under a scratch directory of yours, then:

```sh
S=/path/to/scratch; mkdir -p "$S"
# $R is the Build block's checkout, and the shell is still in it (the relative
# config/ paths below read the checkout's files).
LLM=( ~/.openalpaca/config/llm.toml )      # every llm.toml you have: add any other OPENALPACA_CONFIG_DIR's
[ -f "$R/config/llm.toml" ] && LLM+=( "$R/config/llm.toml" )   # the checkout's ignored copy, when one was seeded
TOML=( ... )     # every real TOML file, "${LLM[@]}" among them, in a fixed order; keep the list: printf '%s\n' "${TOML[@]}" > $S/toml.list
SKILLS=( ... )   # every real SKILL.md

$B/tomlcmp "${TOML[@]}"                                   > $S/tomlcmp.out
$B/tomlcmp --summary "${TOML[@]}"                         > $S/tomlcmp.summary
$B/tedit-0.25.13 record  --out  $S/tedit.record "${TOML[@]}"     # the OLD binary records
$B/tedit-0.25.17 compare --from $S/tedit.record "${TOML[@]}"            > $S/tedit.out
$B/tedit-0.25.17 compare --from $S/tedit.record --summary   "${TOML[@]}" > $S/tedit.summary
$B/tedit-0.25.17 compare --from $S/tedit.record --roundtrip "${TOML[@]}" > $S/tedit.roundtrip
$B/yamlcmp "${SKILLS[@]}"                                 > $S/yamlcmp.out
$B/yamlcmp --summary "${SKILLS[@]}"                       > $S/yamlcmp.summary

for v in base wave; do      # each probe under both builds (F-08); compare the two outputs line by line
  $B/typedprobe-$v daemon   ~/.openalpaca/config/daemon.toml config/daemon.toml   > $S/tp-$v-daemon.out
  $B/typedprobe-$v mcp      ~/.openalpaca/config/mcp.toml    config/mcp.toml      > $S/tp-$v-mcp.out
  $B/typedprobe-$v llm-edit "${LLM[@]}"                                           > $S/tp-$v-llm-edit.out  # reads; renders the edit in memory; writes nothing
  $B/typedprobe-$v plugin   ~/.openalpaca/plugins/*/plugin.toml                   > $S/tp-$v-plugin.out    # the file names the directory that is probed
  $B/typedprobe-$v skill    "${SKILLS[@]}"                                        > $S/tp-$v-skill.out
  $B/typedprobe-$v agent-md config/agents/*.md                                    > $S/tp-$v-agent-md.out
  $B/typedprobe-$v soul     ~/.openalpaca/config/orchestrator/SOUL.md             > $S/tp-$v-soul.out      # likewise user, identity, bootstrap
done
```

(In zsh a glob that matches nothing is an error — `setopt null_glob` first, or
build that list with `find` like the others.)

`tedit`'s two steps must name the same inputs in the same order; the compare
step refuses a record of another length, and a record whose first line is not
`# tedit record v2` (one written by a build from before 2026-10-17, which had no
`panic` status) — record again with the old binary built from this source. The
record file holds an index, a status word (`ok`, `edit-only`, `parse-error`,
`unreadable` or `panic`), a SHA-256 and two counts per input.

**The DU-10 count** (the keyring gate of P12: does any file point at the
keychain?), counts only, over every file that can hold a keychain reference —
not `llm.toml` alone: the daemon hands the plugin manager its secret store
(`apps/openalpacad/src/main.rs:433`), which writes keychain items named
`openalpaca-plugin-<name>-<key>` (`crates/openalpaca_plugins/src/manager.rs:2292`)
and records each as an inline table, `key = { secret_ref = … }`, in
`plugins/.config/<name>.toml` (`crates/openalpaca_plugins/src/permission_gate.rs:446`),
a spelling the line-anchored grep this README used before 2026-10-17 missed
(review SF-14):

```sh
# LLM is the list set at the top of the run above: the home store's llm.toml, the
# checkout's ignored copy when one was seeded, and any other OPENALPACA_CONFIG_DIR's
# (in a fresh shell it is empty: keycount then prints its usage line, exits 2 and counts nothing).
PLUG=(); while IFS= read -r f; do PLUG+=("$f"); done < <(
  find ~/.openalpaca/plugins/.permissions.toml ~/.openalpaca/plugins/.config -maxdepth 1 -type f -name '*.toml' 2>/dev/null | LC_ALL=C sort)
printf '%s\n' "${LLM[@]}" > $S/keycount-llm.list; printf '%s\n' "${PLUG[@]}" > $S/keycount-plugins.list
$B/keycount "${LLM[@]}"            > $S/keycount-llm.out
$B/keycount --summary "${LLM[@]}"  > $S/keycount-llm.summary
[ ${#PLUG[@]} -gt 0 ] && $B/keycount "${PLUG[@]}"           > $S/keycount-plugins.out
[ ${#PLUG[@]} -gt 0 ] && $B/keycount --summary "${PLUG[@]}" > $S/keycount-plugins.summary
```

P12's waiver of its keychain round trip holds only when every input of both runs
reads `counted` and both summaries read `secret_ref 0` and `use_keychain-true 0`
(no plugin file at all counts as a 0 for the plugin half), over an `llm.toml`
list that holds every copy — the checkout's included when it is present. A
`parse-error`, `unreadable` or `panic` row leaves that file uncounted, so the
waiver does not hold until a person has looked at that file — on the owner's
side, never pasted.

**What to expect.** On the checkout's own files every input reads `equal` and
every probe `ok` (the measurement behind `df477c67`, the `--summary` facets
all 0; again on 2026-10-17 under both typedprobe builds, with the `panic-*`
counts 0), and `keycount` counts 0 and 0 on the `llm.toml` seed. A line where
`typedprobe-base` and `typedprobe-wave` disagree is a file the wave's loaders
read differently, which is what F-08's second build exists to find before any
live start. `tedit --roundtrip` reads `defect-shared` for `daemon.toml` and the
`llm.toml` seed: the four `reparse_equal=false` scenarios of A08 — an empty
table the typed side renders and `apply_delta` carries differently — present in
0.25.13 and 0.25.17 alike, a property of the editor's own code, not of the bump
(other files can read `defect-shared` for the same reason; only
`defect-old-only` or `defect-new-only` is the bump's).

The frontmatter edge cases of the ZCode audit (Z3-05, A-07), measured on
synthetic files on 2026-10-17 — properties of today's sources or of P30, not of
a real file:
- A `SKILL.md` whose last frontmatter key is a block scalar with clip or keep
  chomping (`>`, `|`, `>+`, `|+`) reads `different` on both of yamlcmp's facets:
  the `---` cut leaves that scalar with no final line break, serde_yaml 0.9.34
  then yields no trailing `\n` and serde-saphyr with P30's options yields one
  (1.3.0 on 2026-10-17; 2.0.0 the same on 2026-10-20). Strip chomping (`>-`,
  `|-`), and a block scalar followed by another key or a blank line, read
  `equal`. Both typedprobe builds read all of them `ok`.
- A `SKILL.md` that starts with a byte-order mark reads `no-frontmatter` in
  yamlcmp and `err` under both typedprobe builds: the loader's `trim_start` does
  not remove U+FEFF (`crates/openalpaca_core/src/middleware/skill/parser.rs:6`).
- CRLF line ends, a quoted value containing `: `, a nested map, an unknown key
  (`require_capabilities`) and an empty body, with or without a final line
  break, read `equal` on both facets and `ok` under both typedprobe builds.
  yamlcmp cannot flag an unknown key — both facets keep it on both sides (the
  typed one by name, in `unknown_keys`, since N-SKILL-2) — so that check is
  N-SKILL-2's diagnostic, not E6's.
- A `----` or a `---x` line where the closing `---` belongs, with no other
  `---` line after it, reads `equal` and `ok`: the loader's cut, which yamlcmp
  copies, closes the block there (`parser.rs:17` takes the first `\n---`) and
  both readers accept what it leaves; typedprobe's `ok` is the real loader
  doing the same, as Z3-05 read.

Measured on synthetic files on 2026-10-20, the 2.0.0 build beside the 1.3.0
one, for C1 and C10 (programme-plan §4.5 E6's gate):
- 33 or 40 consecutive comment lines between a key and its block value read
  `equal` on both facets; they read `parse-error-new` on the 1.3.0 build and on
  a 2.0.0 build without `emit_comments: false` (32 lines read `equal` on all
  three). With the option, then, the run shows that such a skill reads as
  serde_yaml read it — P30 carries the same option (D4 (a)) — and not whether
  one exists; that would take a build without the option.
- A non-finite float in a `serde_json::Value` position (`tools.defaults`, a
  script's `parameters`) — `.inf`, `-.inf`, `.nan`, `+.Inf`, `.NaN`, `1e999` —
  reads `different` on both facets, on both builds: serde_yaml read it as null
  (`1e999` as that text), serde-saphyr reads its canonical spelling as a string
  (C10). In a string field the same spellings read `equal` on the typed facet
  and `different` on `--facet value`, where every position is a Value.
- A top-level key written twice read `parse-error-old` on the typed facet and
  `equal` on `--facet value`, on both builds, with that day's copy of the
  struct: serde_yaml hands both entries to `SkillFrontmatter`, which refuses a
  duplicate field, while P30's `DuplicateKeyPolicy::LastWins` kept the last
  before the struct saw it. *Superseded on 2026-10-27, below.* YAML 1.1
  spellings (`yes`, `no`, `on`, `off`, `y`, `n`), a `<<` key with an alias, a
  key written twice inside `tools.defaults`, a `!!binary` string and a minimal
  skill read `equal` on both facets.

Measured on synthetic files on 2026-10-27 with yamlcmp on today's struct —
`HEAD`'s `SkillFrontmatter`, which since N-SKILL-2 (`8c63804f`) has
`#[serde(flatten)] unknown_keys`, so serde reads the frontmatter as a map and
buffers every unknown key's value — the struct `consent-point.sh` builds
yamlcmp on (*One command*); the committed copy predates it and gives the
2026-10-20 words:
- A top-level key written twice (`description`, `name`) reads
  `parse-error-both` on the typed facet and `equal` on `--facet value`:
  serde-saphyr 2.0.0 refuses it too through the flatten, so it is a file the
  tree refuses today and P30 refuses as well. typedprobe-base reads it `err`;
  typedprobe-wave, the prep tree's struct from before the flatten on 1.3.0,
  reads it `ok` — a `tp-skill <i> base=err wave=ok` that is the wave's older
  struct, not the bump.
- An unknown key whose value is a custom tag (`!custom v`, `!local {a: 1}`,
  `!t [1, 2]`), `!!null ""` or an integer beyond 64 bits reads
  `parse-error-old` on both facets, `err` under typedprobe-base and `ok` under
  typedprobe-wave: serde_yaml refuses such a value wherever it must read it
  with no type to read it into (the flatten's buffer, a `serde_json::Value`)
  and serde-saphyr reads it, so today's loader refuses the file and P30 would
  load it. `!!bool yes` reads `parse-error-both` and `err` on both builds. A
  plain, nested, null, float, i64 or u64 value on an unknown key, and
  `require_capabilities: [a]`, read `equal` on both facets and `ok` on both
  builds.

Report such a result as what it is. A `different`, a `parse-error-*` or a
`panic-*` on a real file is what the run exists to find: report its index and
the summary facet that moved, never the file's content.

**E6's corpus is pending.** The fixtures above are not committed. A-07 puts
them, written fresh, in P30's pin test and in E6's corpus (plan-rev3, P30, its
rev 3.14 item (2)), with the gate line "no new skill diagnostic over the E6
corpus" once N-SKILL-2 has landed (item (5)); P30 commits them. Committed
beside these tools, every fixture `.md` needs a `!path` line in `.gitignore`'s
`# Markdown Documentation (Except Manuals)` block, or it stays untracked
(`*.md` is ignored there; `git check-ignore -q <file>` exits 0 for one that is).

**Prove the run wrote nothing** — over `config/` **and** `plugins/`, which the
run also reads (review F-13); the snapshot files hold names and hashes, so they
stay on your side:

```sh
snap() { for d in ~/.openalpaca/config ~/.openalpaca/plugins; do
           [ -d "$d" ] || { echo "absent $d"; continue; }
           ( cd "$d" && find . -type f -print0 | sort -z | xargs -0 shasum -a 256; find . | wc -l ); done; }
snap > $S/before.txt
# … the runs …
snap > $S/after.txt
cmp $S/before.txt $S/after.txt && echo unchanged
```

(As `df477c67` did on a synthetic directory holding the `shipped/skill-…` path
that triggered the old serializer branch, and as the 2026-10-17 revision did on
a synthetic store's `config/` and `plugins/`.)

## Proving the panic path

Each tool installs a panic hook that prints one fixed line on stderr —
`<tool>: a panic; its message is withheld because it could quote the input` —
and never the panic's own message, and catches a panic per input
(`catch_unwind`), so a panic inside a library is an outcome, not a message: the
comparators' `panic-old`, `panic-new` or `panic-both`, typedprobe's and
keycount's `panic` (review F-13; before 2026-10-17 only `yamlcmp` had a hook, a
silent one, and its per-input word for a panic was the parse-error word, though
its `--summary` counted panics apart). A panic outside
that catch still prints only the fixed line, and the process exits 101: a lost
run, never a quoted input.

`E6_SELFTEST_PANIC=<index>[:old|new]` makes the named input panic on purpose,
with its path and its text as the panic's payload — what a library quoting its
input would do. `tomlcmp` and `yamlcmp` hold both sides: no suffix panics both,
`:old` or `:new` one. `tedit`, `typedprobe` and `keycount` are one side per
binary and ignore the suffix; for `tedit`, set it on `record` for `panic-old`,
on `compare` for `panic-new`, on both for `panic-both`. A value of another shape
exits 2 before any input is read. Never set it on a real run.

Before the owner-side run, prove on a synthetic file whose path and text hold a
marker that the marker reaches neither stream:

```sh
D=$X/fx/MARKER-7f3a; mkdir -p "$D"
printf 'k = "MARKER-7f3a"\n' > "$D/a.toml"
printf -- '---\nid: m\nname: m\ndescription: MARKER-7f3a\n---\nMARKER-7f3a\n' > "$D/SKILL.md"
: > $X/p.out; : > $X/p.err
for side in "" :old :new; do
  E6_SELFTEST_PANIC=1$side $B/tomlcmp "$D/a.toml"   >> $X/p.out 2>> $X/p.err
  E6_SELFTEST_PANIC=1$side $B/yamlcmp "$D/SKILL.md" >> $X/p.out 2>> $X/p.err
done
E6_SELFTEST_PANIC=1 $B/tedit-0.25.13 record --out $X/p.record "$D/a.toml" >> $X/p.out 2>> $X/p.err
E6_SELFTEST_PANIC=1 $B/tedit-0.25.17 compare --from $X/p.record "$D/a.toml" >> $X/p.out 2>> $X/p.err
E6_SELFTEST_PANIC=1 $B/keycount "$D/a.toml" >> $X/p.out 2>> $X/p.err
for v in base wave; do E6_SELFTEST_PANIC=1 $B/typedprobe-$v skill "$D/SKILL.md" >> $X/p.out 2>> $X/p.err; done
cat $X/p.out                                       # panic-both, panic-both, panic-old, panic-old, panic-new, panic-new,
                                                   # panic-both (tedit), panic - - (keycount), panic, panic — each at index 1
grep -c MARKER-7f3a $X/p.out $X/p.err $X/p.record  # 0 for each
grep -c -v -F 'a panic; its message is withheld' $X/p.err   # 0
```

(`grep -c` exits 1 on a zero count; the number printed is still the answer.)

## What the tools do not do

- They never print a value, a key, a path, an error message or a hash on stdout
  (`tedit record` writes hashes to the file you name, and nothing to stdout);
  `keycount`'s summary names the two keys it counts, `secret_ref` and
  `use_keychain`, and no other.
- On stderr they print a usage line, a refusal of a record file or of the
  self-test knob, or the fixed panic line — never anything taken from an input.
- They never write beside an input, never walk a directory, never follow a glob of their own.
- `yamlcmp`'s typed facet is about `SKILL.md`. An agent template or a persona
  document is parsed by hand-written scanners in the tree, not by YAML
  (`agent/template/mod.rs`, `middleware/persona_frontmatter.rs`); for those,
  `typedprobe agent-md` / `soul|user|identity|bootstrap` is the check, and
  `yamlcmp --facet value` is informative only.
- `tedit` keeps `toml` at 0.9.12 on both sides: it isolates the `toml_edit`
  move. The `toml` move is `tomlcmp`'s.

## Revision log

| Date | Commit | Change |
|---|---|---|
| 2026-10-11 | `df477c67` | The four comparators rebuilt from the September tools (R13): one source per pair, index-and-word output, nothing walked or written. |
| 2026-10-17 | `7d33b59c` | The 2026-10-17 plan review's leftovers for the tools (`../../reports/2026-10-17-plan-review.md`; plan-rev3 §5.1 item 3 and §6 E6 at rev 3.13). **F-13:** `tomlcmp`, `tedit`, `typedprobe` — and `keycount` — catch a panic per input and print one fixed stderr line in place of its message, a panic reported as `panic-old`/`panic-new`/`panic-both` (typedprobe and keycount `panic`) and counted as a panic only; `yamlcmp` likewise, where a panic's per-input word had been the parse-error word (its `--summary` already counted panics apart) under a silent hook; `tedit`'s record file is `v2` (a `panic` status) and refuses a v1 record; the `E6_SELFTEST_PANIC` knob and *Proving the panic path*; the nothing-written proof hashes `plugins/` beside `config/`. **SF-14:** `keycount`, a TOML-parser count of `secret_ref` keys at any depth and of `use_keychain = true`, over `llm.toml`, `plugins/.config/*.toml` and `plugins/.permissions.toml`, counts only, in place of the grep loop; P12's waiver rule. **A2-16:** the toolchain read from each tree's `rust-toolchain.toml` (`tc`), never `+1.93.0`. **MF-5:** `CARGO_BUILD_BUILD_DIR` beside `CARGO_TARGET_DIR` in `c()`. **F-08:** typedprobe built from the branch point and from the prep tree, its `toml` line taken from the tree, and run under both. `tomlcmp`'s `=1.1.6` pin unchanged (its re-pin to 1.1.8 is a cut-day step). Verified in a scratch directory over synthetic fixtures only: every tool built with `c()` (1.93.0; typedprobe-wave 1.99.0 against the pinned prep tree, toml 1.1.8), 51 checks — output shape, a marker in a fixture's path and text absent from every stream, each tool's panic word on each side, tedit's v1 and wrong-length refusals, keycount's counts in every spelling (3 in a plugin config where the old grep saw 1), the snapshot of `config/` and `plugins/` unchanged, nothing under `$CARGO_HOME/build`. **Second revise — the ZCode audit (`../../../research/zcode-audit-2026-10-17.md`: Z3-05, A-07) and its verifier's findings:** the DU-10 count, `typedprobe llm-edit` and the comparators' TOML list take the checkout's ignored `config/llm.toml` when one is present — a daemon started from the checkout seeds it there through the config walk-up, not `OPENALPACA_CONFIG_DIR` — so P12's waiver counts every copy, as plan-rev3 §5.1 item 3's "both copies" asks (`LLM` is set once, at the top of the run); the re-pin paragraph framed by the cut instant, pending D1, its 1.1.8 read from the prep tip's lock; *What to expect* records the frontmatter edge cases measured on synthetic files (block scalars by chomping and position, BOM, CRLF, a quoted `: `, a nested map, an unknown key, an empty body, a `----` or `---x` closing line), and says E6's corpus is P30's and not committed (A-07; a `!path` line in `.gitignore` for its `.md` files). No tool source changed in this pass. Verified in a scratch directory, synthetic files only and never the checkout's `config/llm.toml`: the Build block as written (1.93.0; typedprobe-wave 1.99.0 against prep tip `7e702582`, toml 1.1.8; `comm` printed only `typedprobe 0.1.0` twice); the 51 checks again, 51 passed; the panic proof's documented ten lines under bash and zsh; the run, DU-10 and snapshot blocks under bash and `zsh -f` against a synthetic store and two fake checkouts, with and without a seeded `config/llm.toml` (the `llm.toml` list 2 and 1 inputs, the planted keychain reference counted, `llm-edit` `ok` on each, base and wave agreeing, the store and both checkouts unchanged; with `LLM` empty, as in a fresh shell, keycount prints its usage line and exits 2); 19 edge-case fixtures under yamlcmp's two facets and both typedprobe builds, stderr empty; rustfmt on the five sources and clippy `-D warnings` on tomlcmp, yamlcmp, keycount and tedit's two manifests clean; the e6 tree, the root lock, `target/` and `$CARGO_HOME/build` untouched. |
| 2026-10-20 | `2c913f99` | The pre-cut tool amendments (programme-plan §4.5 E6 and §6.8; `tasks/todo.md` Chunk 25). **D4 (a):** `yamlcmp` on `serde-saphyr = "=2.0.0"`, with `non_finite_float_policy: AsString` in place of the deprecated `reject_non_finite_typeless_float` (the same reading: 2.0.0 maps the old `false` to `AsString`), the module doc and `yaml_options()`'s doc comment rewritten (in a `serde_json::Value` position a non-finite float becomes its canonical spelling as a string, C10); its `Cargo.lock` moved serde-saphyr and granit-parser 1.3.0 → 2.0.0 and nothing else. **C1:** `emit_comments: false` in `yaml_options()`. **P29:** `tomlcmp`'s `toml11` at `=1.1.8`; its `Cargo.lock` moved toml 1.1.6 → 1.1.8 and nothing else (its toml_parser 1.1.5, toml_writer 1.1.3, toml_datetime 1.1.2, serde_spanned 1.1.2 and winnow 1.0.4 are already the measured final lock's). The re-pin paragraph is now a record; the intro, the two table rows and the block-scalar bullet name the new versions; *What to expect* records the C1, C10 and duplicate-key measurements; the 2026-10-17 row's commit is filled in (`7d33b59c`). Verified in a scratch directory, on the checkout's shipped files and synthetic files only: both tools built with `c()` on 1.93.0, once without `--locked` and again `--locked` from an empty target dir, 0 warnings (a 2.0.0 build keeping the old field warns `use of deprecated field`); clippy `-D warnings` and rustfmt clean; the four repository skills `equal` on both facets; tomlcmp `equal` on seven checkout files (`config/daemon.toml`, `config/mcp.toml`, `config/tools/example.toml`, the three seeds under `scripts/release/templates/config/`, `tests/fixtures/echo-plugin/plugin.toml`), every facet and cross-read count 0, as on the 1.1.6 build; 22 synthetic frontmatters under the 2.0.0 and 1.3.0 builds and 16 of them under a 2.0.0 build without `emit_comments: false` (the words in *What to expect*); stderr empty on every run; nothing under `config/`, `scripts/` or `tests/` newer than a stamp taken before the runs, and `git status` changed by these files alone. |
| 2026-10-27 | (to fill) | **`consent-point.sh` and `consent-point-test.sh`** — the consent point as one owner-run command, after the owner's choice (a) of 2026-10-27 (PDT; 2026-10-28T04:21Z) to run it now (programme-plan §5 item 3); the section *One command*. The script builds the Build block's tools from `git archive` exports of `--base` and `--wave`. It builds the input lists the way the daemon loads its files and runs the run block and the DU-10 count. It prints a paste block of commits, versions, counts, indexes and fixed words, and nothing taken from a file. Hardened after its first review: `LC_ALL=C` globally and in the block's filter (under a UTF-8 locale a bracket range let letters such as ü through); a per-tool time limit (`--tool-time-limit`, SIGALRM through perl, `<step>-timeout`); fds 3 and 4 closed for every child that runs outside code; an inherited xtrace, verbose, DEBUG trap, `CDPATH`, `GIT_DIR` and `GIT_TRACE` neutralised; an empty `RUSTC_WRAPPER` / `RUSTC_WORKSPACE_WRAPPER` and no `MAKEFLAGS` for cargo; the build tree removed on every exit, Ctrl-C included (130, TERM 143, hangup 129); a directory the run cannot enter or list counted `skipped`; `inputs-unchanged` over every fed file read through links, and modes and mtimes in `store-unchanged`; the waiver's `skipped-input`; the checkout's working-tree files that differ from the export counted (`checkout-config-local`) and fed under the flag; `same-as-checkout`; `skill-md-not-loaded-by-scan`; `base-matches-old-pins` (a base past P04 fails as `base-past-old-pins`); and serde-saphyr and granit-parser on each typedprobe lock line, with `wave-matches-new-pins`, because the default wave holds serde-saphyr 1.3.0. After its second review: yamlcmp built on the base's own `SkillFrontmatter`, derived from the `--base` export with the copy's two documented changes (`yamlcmp-struct`; the committed copy lacks N-SKILL-2's `#[serde(flatten)] unknown_keys`, `8c63804f`, and its refresh is a tool amendment of its own, still to do); the duplicate-key bullet of *What to expect* re-measured on that struct; `wave-loader-behind-base`, so that a typedprobe disagreement is laid to the bump only where the base's loader did not change; `old-side-stack-matches-base` (informational; the cut's toml_parser, toml_writer, toml_datetime, serde_spanned and indexmap compared with today's at the source, no behaviour changed); the toolchain check run from `/` with `RUSTUP_AUTO_INSTALL=0` and no `RUSTUP_TOOLCHAIN` (rustup 1.29 installs the active toolchain on `rustup toolchain list`); the store's snapshot taken before the exports and the fed files' before the builds (the lists now come first), both with ctime, flags and inode, so a write during the builds or a change to metadata alone reads `no`; the scratch-dir line on stderr, so stdout is the block alone; a closed stdout or stderr and a failed mktemp stopped with the fixed text and no scratch dir left; a scratch root inside `--home`'s `config` or `plugins` through a link refused, and `--home /`; the build tree's removal retried after Ctrl-C. Verified in a scratch directory on synthetic stores only, under /bin/bash 3.2.57: the test, 146 passed and 0 failed — on every run no sentinel and no fixture path on either stream, stdout the block alone and stderr the one scratch-dir line; `store-unchanged yes` and `inputs-unchanged yes` on the clean runs, with the fixture unchanged by an independent hash-and-stat check; an extended attribute set during the builds on a store file and on a fed file outside the store, bytes, mode and mtime unchanged, read `no` on both lines; the duplicated-key and tagged-key skills read `parse-error-both` and `parse-error-old`; a working-tree pin that is not installed passed the pre-flight, every rustup call from `/` under `RUSTUP_AUTO_INSTALL=0`; the Ctrl-C run's build tree removed on the second attempt; the closed streams, the unwritable scratch root, the links into the store and `--home /` refused with the fixed text and no scratch dir left. The derivation applied to the merge base's `types.rs` gives the committed copy byte for byte; 15 synthetic frontmatters under the derived yamlcmp and both typedprobe builds give the words in *What to expect*, stderr empty; the derived yamlcmp builds `--locked` with no warning; shellcheck is clean on both scripts. |
