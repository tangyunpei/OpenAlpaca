# E6 — the read-only comparators for the owner's real config and skill files

The dependency wave moves the three crates that read the owner's configuration:
`toml` 0.9.12 → 1.1 (P29; 1.1.8 at the cut — *Re-pin* below), `toml_edit` 0.25.13 → 0.25.17 (P04's lock
refresh) and `serde_yaml` 0.9.34 → `serde-saphyr` 1.3.0 with serde_yaml-compatible
options (P30). Plan-rev3 §6 E6 asks, **before P04, P12, P29 and P30**, whether
the owner's real files read the same under both versions — with tools that print
only an index and an equality bit, never a value, a key or a path, and that
cannot write.

These four crates are those tools, rebuilt from the September ones (R13: the
archived `tedit-0.25.15` carried a diagnostic its 0.25.13 twin lacked, and the
archived `yamlcmp` wrote `render-*.yaml` beside its corpus and walked every
`.md`). Each is one source; the old/new pair of `tedit` is one `src/main.rs`
under two manifests that differ in the `toml_edit` line alone (`diff
tedit/old/Cargo.toml tedit/new/Cargo.toml`).

| Tool | Compares | Loader it mirrors |
|---|---|---|
| `tomlcmp` | toml 0.9.12 vs 1.1.6 as committed (1.1.8 for the run — *Re-pin* below), in one binary | `toml::from_str::<Value>`, `to_string`, `to_string_pretty` (`render_config`), the leaf `Display` (`config_codec::toml_to_json`) |
| `tedit` (`old/`, `new/`) | toml_edit 0.25.13 vs 0.25.17, two binaries | `render_config_preserving`'s `apply_delta` (copied verbatim from `crates/openalpaca_llm/src/config/llm_config/edit.rs:74-184`), with the September mutation set (six seeds, `s * 7919 + 17`) |
| `yamlcmp` | serde_yaml 0.9.34 vs serde-saphyr 1.3.0 + P30's options, in one binary | `parse_skill_frontmatter` (`middleware/skill/mod.rs:51-56`): the frontmatter into `SkillFrontmatter` (a copy of `middleware/skill/types.rs`), validated |
| `typedprobe` | nothing — it runs the **current source's** loaders by path | `load_daemon_config`, the M3 `llm.toml` edit, `McpConfig::load`, `AgentConfigFile`, `PluginManifest::from_dir_for_load`, `parse_skill_markdown`, `parse_agent_markdown`, the four persona parsers |

Every tool takes an explicit list of files on its command line (nothing is
walked), prints one line per input — `<index>\t<word>`, the index the input's
1-based position in the argument list — and exits 0. `--summary` prints counts
only. The words:

- `equal` / `different` — both versions read the file; the result is the same / not.
- `parse-error-old` / `parse-error-new` / `parse-error-both` — one version or both refused the file.
- `unreadable` — the file could not be read as UTF-8 text; `no-frontmatter` (yamlcmp) — no `---` block.
- typedprobe prints `ok` or `err` instead: whether the real loader accepted the file (for `daemon` and `llm-edit` with extra conditions — see the source's header).

Keep your argument list in a file so that an index can be looked up on your
side; the tools never print the path.

## Build (once, with the pinned toolchain, into a target directory of your choosing)

```sh
T=/path/to/a/target/dir            # not the repository's target/
E=tasks/deps-upgrade/tools/e6
B=/path/to/a/scratch/bin; mkdir -p "$B"
c() { CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="$T" cargo +1.93.0 build --manifest-path "$1"; }
c $E/tomlcmp/Cargo.toml   && cp "$T/debug/tomlcmp" "$B/"
c $E/yamlcmp/Cargo.toml   && cp "$T/debug/yamlcmp" "$B/"
c $E/tedit/old/Cargo.toml && cp "$T/debug/tedit" "$B/tedit-0.25.13"
c $E/tedit/new/Cargo.toml && cp "$T/debug/tedit" "$B/tedit-0.25.17"
# typedprobe builds from a scratch copy beside a copy of the checkout's lock (core
# resolves to the daemon's versions), its three path lines pointed at this
# checkout, so nothing is written inside the checkout, even by a build cut short
R=$(git rev-parse --show-toplevel)
P=/path/to/a/scratch/typedprobe; rm -rf "$P"; mkdir -p "$P"
cp -R $E/typedprobe/src "$P/" && cp "$R/Cargo.lock" "$P/Cargo.lock"
sed "s|\.\./\.\./\.\./\.\./\.\./crates/|$R/crates/|" $E/typedprobe/Cargo.toml > "$P/Cargo.toml"
c "$P/Cargo.toml" && cp "$T/debug/typedprobe" "$B/"
```

The three comparators carry their own `Cargo.lock` (the exact `toml_parser`,
`winnow`, `granit-parser` the comparison ran on); `typedprobe` takes the
checkout's, because the question it answers is about the daemon's own graph.
`typedprobe` compiles `openalpaca_core`, `_llm` and `_plugins` from the checkout
`R` names — build it from the commit whose loaders you want to probe (before
P29, `toml = "0.9"` in its manifest follows the workspace; P29 moves both).

**Re-pin `tomlcmp` before the owner-side run.** The pins are the 7-day rule's
answers on 2026-10-11: `toml11 = "=1.1.6"` (`tomlcmp/Cargo.toml`), `toml_edit =
"=0.25.17"` (`tedit/new/Cargo.toml`), `serde-saphyr = "=1.3.0"`
(`yamlcmp/Cargo.toml`). The cut's lock — on or after 2026-10-17 under DU-23 (a) —
holds **toml 1.1.8** (eligible from 2026-10-16; the measured final lock,
`../gbase/locks/recut-lock-99-final.lock`, holds `1.1.8+spec-1.1.0`), while
its toml_edit 0.25.17 and serde-saphyr 1.3.0 are these pins. So before the run,
change `tomlcmp/Cargo.toml`'s `toml11` line to `=1.1.8`, rebuild tomlcmp (its
`Cargo.lock` follows) and run that build: the comparison must be against the
version the daemon will be built with, and a run on 1.1.6 answers nothing about
the wave. If the G-base run on the day prints another version for any of the
three, the same rule: that one line takes what the lock holds.

## The owner-side run (plan-rev3 §5.1 item 3, §6 E6)

**The files.** `llm.toml`, `daemon.toml`, `mcp.toml` in `~/.openalpaca/config/`
and in the checkout's `config/` (the checkout has no `llm.toml`; the seed is
`scripts/release/templates/config/llm.toml`); any other `OPENALPACA_CONFIG_DIR`;
`~/.openalpaca/plugins/.permissions.toml`, `plugins/.config/*.toml`, each
`plugins/*/plugin.toml`; `config/tools/*.toml`; any legacy `config/agents/*.toml`;
every `SKILL.md` under the skill directories, plugin-contributed ones included
(`find <dir> -name SKILL.md`, then pass the list — the tool itself walks nothing).

Write each list to a file under a scratch directory of yours, then:

```sh
S=/path/to/scratch; mkdir -p "$S"
TOML=( ... )     # every real TOML file, in a fixed order; keep the list: printf '%s\n' "${TOML[@]}" > $S/toml.list
SKILLS=( ... )   # every real SKILL.md

$B/tomlcmp "${TOML[@]}"                                   > $S/tomlcmp.out
$B/tomlcmp --summary "${TOML[@]}"                         > $S/tomlcmp.summary
$B/tedit-0.25.13 record  --out  $S/tedit.record "${TOML[@]}"     # the OLD binary records
$B/tedit-0.25.17 compare --from $S/tedit.record "${TOML[@]}"            > $S/tedit.out
$B/tedit-0.25.17 compare --from $S/tedit.record --summary   "${TOML[@]}" > $S/tedit.summary
$B/tedit-0.25.17 compare --from $S/tedit.record --roundtrip "${TOML[@]}" > $S/tedit.roundtrip
$B/yamlcmp "${SKILLS[@]}"                                 > $S/yamlcmp.out
$B/yamlcmp --summary "${SKILLS[@]}"                       > $S/yamlcmp.summary

$B/typedprobe daemon   ~/.openalpaca/config/daemon.toml config/daemon.toml
$B/typedprobe mcp      ~/.openalpaca/config/mcp.toml    config/mcp.toml
$B/typedprobe llm-edit ~/.openalpaca/config/llm.toml                      # reads; renders the edit in memory; writes nothing
$B/typedprobe plugin   ~/.openalpaca/plugins/*/plugin.toml                # the file names the directory that is probed
$B/typedprobe skill    "${SKILLS[@]}"
$B/typedprobe agent-md config/agents/*.md
$B/typedprobe soul     ~/.openalpaca/config/orchestrator/SOUL.md          # likewise user, identity, bootstrap
```

`tedit`'s two steps must name the same inputs in the same order; the compare
step refuses a record of another length. The record file holds an index, a
status word, a SHA-256 and two counts per input.

**What to expect.** On the checkout's own files every input reads `equal` and
every probe `ok` (the measurement behind this commit; the `--summary` facets
all 0). `tedit --roundtrip` reads `defect-shared` for `daemon.toml` and the
`llm.toml` seed: the four `reparse_equal=false` scenarios of A08 — an empty
table the typed side renders and `apply_delta` carries differently — present in
0.25.13 and 0.25.17 alike, a property of the editor's own code, not of the bump.
A `different` or a `parse-error-*` on a real file is what the run exists to find:
report its index and the summary facet that moved, never the file's content.

**Prove the run wrote nothing** (as this commit did on a synthetic directory
holding the `shipped/skill-…` path that triggered the old serializer branch):

```sh
( cd ~/.openalpaca/config && find . -type f | sort | xargs shasum -a 256; find . | wc -l ) > $S/before.txt
# … the runs …
( cd ~/.openalpaca/config && find . -type f | sort | xargs shasum -a 256; find . | wc -l ) > $S/after.txt
cmp $S/before.txt $S/after.txt && echo unchanged
```

**The DU-10 count** (keyring 4 gate: does any `llm.toml` point at the keychain?),
counts only, over every `llm.toml` you have:

```sh
for f in ~/.openalpaca/config/llm.toml ${OPENALPACA_CONFIG_DIR:+$OPENALPACA_CONFIG_DIR/llm.toml}; do
  printf 'secret_ref lines: %s\tuse_keychain=true lines: %s\n' \
    "$(grep -c -E '^[[:space:]]*secret_ref[[:space:]]*=' "$f")" \
    "$(grep -c -E '^[[:space:]]*use_keychain[[:space:]]*=[[:space:]]*true' "$f")"
done
```

(`grep -c` exits 1 on a zero count; the number printed is still the answer.)

## What the tools do not do

- They never print a value, a key, a path, an error message or a hash on stdout
  (`tedit record` writes hashes to the file you name, and nothing to stdout).
- They never write beside an input, never walk a directory, never follow a glob of their own.
- `yamlcmp`'s typed facet is about `SKILL.md`. An agent template or a persona
  document is parsed by hand-written scanners in the tree, not by YAML
  (`agent/template/mod.rs`, `middleware/persona_frontmatter.rs`); for those,
  `typedprobe agent-md` / `soul|user|identity|bootstrap` is the check, and
  `yamlcmp --facet value` is informative only.
- `tedit` keeps `toml` at 0.9.12 on both sides: it isolates the `toml_edit`
  move. The `toml` move is `tomlcmp`'s.
