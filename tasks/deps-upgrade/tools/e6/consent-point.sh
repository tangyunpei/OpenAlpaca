#!/bin/bash
set +o xtrace +o verbose   # an inherited xtrace or verbose (SHELLOPTS, BASH_ENV) is off before any line holding an argument runs
# consent-point.sh — programme-plan §5 item 3's consent point as one owner-run
# command: the DU-10 count and E6's no-print checks over the owner's real
# config and skill files, with the tools of this directory (README.md here,
# *One command*).
#
#   /bin/bash tasks/deps-upgrade/tools/e6/consent-point.sh --home ~/.openalpaca --include-checkout-config
#
# Quit the app and the daemon first (or leave them idle): `store-unchanged`
# and `inputs-unchanged` read a write by the daemon, a running plugin or the
# Finder during the run as a change — a Finder tag or any other extended
# attribute, a flag or an ACL among them (each moves the ctime). The store's
# snapshot is taken before the exports and again after the runs, so its window
# holds the builds (third-party build scripts) as well as the tool runs; the
# fed files' snapshot is taken once the lists exist, before the builds, and
# again after the runs. The run takes about a minute or two with a warm cargo
# cache, nearly all of it the builds, and longer when cargo must first fetch
# the wave's crates. When it ends, stdout holds the block and nothing else:
# paste everything between `=== PASTE FROM HERE ===` and `=== TO HERE ===` into
# the chat (`| pbcopy` takes exactly that). The block holds only commits,
# toolchains, crate versions, counts, indexes and the tools' fixed words —
# never a file name, a path, a key, a value or a byte of a file. One line on
# stderr after the block names the scratch directory: it holds the index lists
# that map each index back to its file, the tools' outputs, the logs, and the
# snapshots — your file paths and the SHA-256 of every file the run read and of
# every file under <home>/config and <home>/plugins. It is not for pasting;
# remove it when done.
#
# Options
#   --home <dir>        Required, no default: the store to read (the owner
#                       passes ~/.openalpaca). Only its config/ and plugins/
#                       are read, and nothing inside it is ever written.
#   --base <commit>     The tree whose loaders the daemon runs today; default
#                       HEAD. The comparators' sources and the base tree are
#                       taken from it with `git archive`, never from the
#                       working tree, so an uncommitted edit (a refreshed
#                       Cargo.lock) cannot reach the base build; the checkout's
#                       tracked config files are read from that export too.
#                       The block's `base-matches-old-pins` line checks it:
#                       typedprobe-base's toml, toml_edit and serde_yaml must be
#                       the versions keycount, tedit-old and yamlcmp's old side
#                       pin (0.9.12, 0.25.13, 0.9.34 — the tree before P04). A
#                       base past P04's lock refresh fails as the step
#                       `base-past-old-pins`: run again with
#                       --base <the commit before P04>. The
#                       `old-side-stack-matches-base` lines (informational)
#                       say, per old-side tool, whether the TOML crates under
#                       those three (toml_parser, toml_writer, toml_datetime,
#                       serde_spanned, indexmap, winnow) are the base's too.
#                       yamlcmp's SkillFrontmatter is taken from this tree
#                       (`yamlcmp-struct source=base`), not from the tool's
#                       committed copy, so its typed facet always reads with
#                       the struct the daemon reads with today.
#   --wave <ref>        The tree with the wave's manifests and lock; default
#                       refs/prep/deps-wave-2026-10-12 (toml 1.1.8, toml_edit
#                       0.25.17 — the cut's — but serde-saphyr 1.3.0, not the
#                       cut's 2.0.0: no ref carries both). The block's
#                       `wave-matches-new-pins` line says which crates are the
#                       comparators' new sides, and `wave-loader-behind-base`
#                       which probed loaders --base changed since the two trees
#                       parted (their merge base): typedprobe-wave runs the
#                       wave's own, older copy of those. A `tp-<kind>`
#                       disagreement is the crate bump's only for a kind that
#                       line marks `no`. On the YAML axis yamlcmp's typed row is
#                       the word of record (the base's struct on serde-saphyr
#                       2.0.0 with P30's options and C1, which is P30): read a
#                       `tp-skill <i> base=... wave=...` row against yamlcmp's
#                       row for <i> (README, *One command*).
#   --include-checkout-config
#                       Also read what a daemon started from the checkout reads
#                       there and the export does not hold byte for byte: every
#                       file of a loaded kind in the checkout's config/ (below)
#                       that is untracked, git-ignored, modified, or changed
#                       since --base — today its config/llm.toml and the four
#                       generated persona documents — and, when
#                       $OPENALPACA_CONFIG_DIR names an existing directory other
#                       than <home>/config and the checkout's config/, that
#                       directory as a whole config directory. P12's keychain
#                       waiver needs every llm.toml counted, so without this
#                       flag it cannot hold while the checkout holds a
#                       config/llm.toml (README, *The DU-10 count*). Without the
#                       flag those checkout files are counted
#                       (`checkout-config-local files=<n> included=no`; a file
#                       the export lacks is only stat'ed), never read.
#   --keep              Keep the build tree too (the two exports and the cargo
#                       target dir, about 3.5 GB). Without it the build tree is
#                       removed when the run ends, however it ends; the lists,
#                       outputs, snapshots, logs and binaries are always kept,
#                       because the index lists are the only map from an index
#                       back to a file.
#   --tool-time-limit <seconds>
#                       The wall-clock limit of each tool run; default 300. A
#                       run past it is killed and fails as `<step>-timeout`
#                       (a file whose shape makes a tool's time grow with the
#                       square of its size — toml_edit's key removal, libyaml's
#                       simple keys — would otherwise hold the run with no end).
#                       Real stores take well under a second per run. The
#                       builds have no limit; Ctrl-C stops them.
#
# What it does, in order
#   1. Pre-flight: the environment made neutral (LC_ALL=C, so every byte
#      comparison is a byte comparison; CDPATH, GIT_DIR, GIT_WORK_TREE and the
#      GIT_TRACE family unset; any inherited DEBUG, ERR or RETURN trap removed);
#      the arguments; --home a directory; the scratch root outside --home (and
#      outside what <home>/config and <home>/plugins resolve to through a link)
#      and outside the checkout; both refs resolved to commits; stdout and
#      stderr open; perl present (it bounds each tool run); each tree's
#      toolchain read from its rust-toolchain.toml and checked with
#      `rustup toolchain list`, run from / with RUSTUP_AUTO_INSTALL=0 and no
#      RUSTUP_TOOLCHAIN, so that neither a working tree's pin nor the
#      environment can make the check install one (never installed: a missing
#      one stops the run).
#   2. A fresh `mktemp -d` scratch directory under ${TMPDIR:-/tmp}; from here
#      every byte of build output, every tool's stdout and stderr, and the
#      script's own streams go to files there. Only the block (on stdout) and
#      the line after it (on stderr) reach the terminal; no child gets the
#      terminal on any descriptor (fds 3 and 4, the block's, are closed for
#      each).
#   3. The store's snapshot (step 6); `git archive` exports of --base and
#      --wave; yamlcmp's SkillFrontmatter derived from the base export's
#      middleware/skill/types.rs with the copy's two documented changes (no
#      `impl ScriptConfig`; `validate()` returning a String), each anchor
#      matched exactly once, or the step fails as `yamlcmp-struct-derive`;
#      `wave-loader-behind-base` from `git diff --name-only` between two
#      commits (objects only).
#   4. The input lists, each LC_ALL=C sorted within a fixed order (README, *The
#      owner-side run*), then the fed files' snapshot, both before any build;
#      which directories the daemon loads is checked in the
#      code — apps/openalpacad/src/services/{mod,agents,tools}.rs, the skill
#      catalog's `scan_directory`, the plugin manager's directory scan,
#      bootstrap/persona.rs. A config directory gives llm.toml, daemon.toml and
#      mcp.toml by name; tools/*.toml; agents/*.toml and agents/*.md;
#      skills/<child>/SKILL.md, one level, as `scan_directory` reads it — the
#      README's `find <dir> -name SKILL.md` would take any depth, and a SKILL.md
#      the scan never loads (flat in skills/ or deeper) is counted as
#      `skill-md-not-loaded-by-scan` and not fed; orchestrator/{SOUL,USER,
#      IDENTITY,BOOTSTRAP}.md. The plugins directory gives .permissions.toml,
#      .config/*.toml and <child>/plugin.toml. A file is fed only when it is a
#      regular file (a link to one is followed); anything else at a loaded
#      path, and a directory the run cannot enter or list, is `skipped`; an
#      absent named file is `missing`.
#   5. The README's Build block: tomlcmp, yamlcmp (on the derived struct),
#      keycount, tedit old and new, each `--locked`; typedprobe twice, from a
#      scratch copy beside each tree's lock, its path lines and its `toml` line
#      taken from that tree, then the `comm` check that no version moved. Every
#      cargo run has RUSTC_WRAPPER and RUSTC_WORKSPACE_WRAPPER cleared (so no
#      wrapper writes a cache outside the scratch dir), MAKEFLAGS unset and
#      RUSTUP_AUTO_INSTALL=0, its target and build dir in scratch (MF-5).
#   6. Two snapshots, each taken twice and kept in the scratch dir:
#      `store-unchanged` (before the exports, after the runs) — every entry
#      under <home>/config and <home>/plugins with its type, mode, mtime,
#      ctime, flags and inode, every regular file's SHA-256, every link's
#      target, the entry count; `inputs-unchanged` (once the lists exist,
#      before the builds; after the runs) — every file fed to a tool, read
#      through links wherever it lives (a skill linked in from outside the
#      store, the checkout's files, $OPENALPACA_CONFIG_DIR's), with its type,
#      mode, mtime, ctime, flags, inode, size and SHA-256. Reading a file never
#      moves its ctime; every change to its bytes or its metadata does.
#   7. Every comparator and probe as the README's run block, plus typedprobe
#      `agent-toml` and yamlcmp's value facet; keycount over the llm.toml list
#      and over the plugin list; P12's waiver as the README defines it, and
#      `does-not-hold (skipped-input)` when a file on either list was skipped.
#   8. The block. Exit 0 when every step ran (a difference is a result, not a
#      failure); 1 when a step could not run (`step-failed <step>` in the
#      block, never the error text); 2 for a usage error before any work;
#      130 after Ctrl-C, 143 after a TERM and 129 after a hangup, the block
#      still printed with `step-failed <step>-interrupted`.
#
# The run never writes inside --home or the checkout (git is used only for
# rev-parse, show, archive, merge-base and a `diff --name-only` between two
# commits, none of which writes the index), never follows a link out of --home
# into a write (it writes nothing there at all), and reaches the network only
# through cargo fetching the builds' crates. It unsets E6_SELFTEST_PANIC for
# the tools. bash 3.2 (/bin/bash on macOS) or later.

# Under sh (macOS's /bin/sh is bash in POSIX mode, which refuses process
# substitution) or another shell, run again under /bin/bash, with nothing of
# the caller's shell start-up carried over.
if [ -z "${BASH_VERSION:-}" ] || (shopt -oq posix) 2>/dev/null; then
  if [ -n "${E6_CONSENT_REEXEC:-}" ]; then
    echo "consent-point: run it with /bin/bash" >&2
    exit 2
  fi
  exec env -u POSIXLY_CORRECT -u BASH_ENV -u ENV -u SHELLOPTS -u BASHOPTS -u BASH_XTRACEFD -u CDPATH \
    E6_CONSENT_REEXEC=1 /bin/bash "$0" "$@"
fi

set -euo pipefail
trap - DEBUG ERR RETURN 2>/dev/null || true
# Byte semantics everywhere: under a UTF-8 locale a bracket range such as
# [A-Za-z] in a `case` pattern matches letters like ü, which would open the
# block's alphabet check. (BASH_XTRACEFD is left alone: xtrace is off, and in
# bash 4.1+ unsetting it when it names fd 2 closes stderr.)
export LC_ALL=C
unset CDPATH GIT_DIR GIT_WORK_TREE GIT_COMMON_DIR GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES \
  GIT_TRACE GIT_TRACE_SETUP GIT_TRACE_PACKET GIT_TRACE_PERFORMANCE GIT_TRACE_PACK_ACCESS GIT_TRACE_CURL \
  GIT_TRACE_SHALLOW GIT_TRACE_REFS GIT_TRACE_FSMONITOR GIT_TRACE2 GIT_TRACE2_EVENT GIT_TRACE2_PERF

PROG=consent-point

die_usage() {
  # A fixed text only: an argument could be a path. With stderr closed the
  # echo fails in silence and the exit status is still 2.
  echo "$PROG: $1" >&2 || true
  echo "usage: $PROG --home <dir> [--base <commit>] [--wave <ref>] [--include-checkout-config] [--keep] [--tool-time-limit <seconds>]" >&2 || true
  exit 2
}

HOME_ARG=""
HOME_SET=0
BASE_REF="HEAD"
WAVE_REF="refs/prep/deps-wave-2026-10-12"
INCLUDE_CHECKOUT=0
KEEP=0
TOOL_LIMIT=300
while [ $# -gt 0 ]; do
  case "$1" in
    --home) [ $# -ge 2 ] || die_usage "--home needs a directory"; HOME_ARG=$2; HOME_SET=1; shift 2 ;;
    --home=*) HOME_ARG=${1#--home=}; HOME_SET=1; shift ;;
    --base) [ $# -ge 2 ] || die_usage "--base needs a commit"; BASE_REF=$2; shift 2 ;;
    --base=*) BASE_REF=${1#--base=}; shift ;;
    --wave) [ $# -ge 2 ] || die_usage "--wave needs a ref"; WAVE_REF=$2; shift 2 ;;
    --wave=*) WAVE_REF=${1#--wave=}; shift ;;
    --include-checkout-config) INCLUDE_CHECKOUT=1; shift ;;
    --keep) KEEP=1; shift ;;
    --tool-time-limit) [ $# -ge 2 ] || die_usage "--tool-time-limit needs a number of seconds"; TOOL_LIMIT=$2; shift 2 ;;
    --tool-time-limit=*) TOOL_LIMIT=${1#--tool-time-limit=}; shift ;;
    -h | --help) die_usage "the consent point's E6 run (header of this script)" ;;
    *) die_usage "unknown argument" ;;
  esac
done
[ "$HOME_SET" = 1 ] && [ -n "$HOME_ARG" ] || die_usage "--home is required (the owner's command passes --home ~/.openalpaca)"
[ -d "$HOME_ARG" ] || die_usage "--home is not a directory"
case "$TOOL_LIMIT" in "" | *[!0-9]* | 0*) die_usage "--tool-time-limit takes a whole number of seconds, 1 to 86400" ;; esac
[ "${#TOOL_LIMIT}" -le 5 ] && [ "$TOOL_LIMIT" -le 86400 ] || die_usage "--tool-time-limit takes a whole number of seconds, 1 to 86400"

# Canonical paths, by `cd -P` (macOS has no reliable `realpath -m`); CDPATH is
# empty for it (an exported one makes `cd` print the directory it found, a
# second line), and a leading '-' cannot read as an option or as $OLDPWD.
canon() {
  local p=$1
  case "$p" in -*) p=./$p ;; esac
  (CDPATH='' cd -P -- "$p" >/dev/null 2>&1 && pwd -P)
}
has_nl() { case "$1" in *$'\n'*) return 0 ;; esac; return 1; }
HOMEC=$(canon "$HOME_ARG") || die_usage "--home cannot be entered"
! has_nl "$HOMEC" || die_usage "--home's path holds a line break"
SCRIPT_DIR=$(canon "$(dirname -- "$0")") || die_usage "cannot find the script's directory"
! has_nl "$SCRIPT_DIR" || die_usage "the script's path holds a line break"
R=$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null </dev/null) || die_usage "the script is not inside a git checkout"
R=$(canon "$R") || die_usage "cannot enter the checkout"
! has_nl "$R" || die_usage "the checkout's path holds a line break"
TMPROOT=$(canon "${TMPDIR:-/tmp}") || die_usage "the scratch root (\$TMPDIR or /tmp) cannot be entered"
! has_nl "$TMPROOT" || die_usage "the scratch root's path holds a line break"
# Whether canonical path $1 is $2 or lies under it; everything lies under /.
inside() {
  [ "$2" != / ] || return 0
  case "$1/" in "$2"/*) return 0 ;; *) return 1 ;; esac
}
# The scratch dir is a fresh directory under the scratch root: it lands inside
# --home or the checkout only when the root is inside one of them — or inside
# what <home>/config or <home>/plugins resolves to, when either is a link to a
# directory elsewhere (the run reads both through links, so a scratch dir there
# would be a write into the store through a link).
inside "$TMPROOT" "$HOMEC" && die_usage "the scratch root (\$TMPDIR or /tmp) is inside --home; set TMPDIR elsewhere"
for sub in config plugins; do
  if [ -d "$HOMEC/$sub" ]; then
    SUBC=$(canon "$HOMEC/$sub") || continue
    ! has_nl "$SUBC" || die_usage "--home's $sub resolves to a path that holds a line break"
    if inside "$TMPROOT" "$SUBC"; then
      die_usage "the scratch root (\$TMPDIR or /tmp) is inside --home's $sub (through a link); set TMPDIR elsewhere"
    fi
  fi
done
inside "$TMPROOT" "$R" && die_usage "the scratch root (\$TMPDIR) is inside the checkout; set TMPDIR elsewhere"
inside "$HOMEC" "$R" && die_usage "--home is inside the checkout"

BASE_SHA=$(git -C "$R" rev-parse --verify --quiet "${BASE_REF}^{commit}" 2>/dev/null </dev/null) || die_usage "--base does not name a commit"
WAVE_SHA=$(git -C "$R" rev-parse --verify --quiet "${WAVE_REF}^{commit}" 2>/dev/null </dev/null) || die_usage "--wave does not name a commit"
is_sha() { case "$1" in *[!0-9a-f]* | "") return 1 ;; esac; [ ${#1} -eq 40 ]; }
if ! is_sha "$BASE_SHA" || ! is_sha "$WAVE_SHA"; then die_usage "a ref did not resolve to a full commit id"; fi

# The toolchain a tree pins (README, `tc`), read from the commit, not a file.
tree_tc() { git -C "$R" show "$1:rust-toolchain.toml" 2>/dev/null </dev/null | sed -n 's/^channel *= *"\(.*\)"$/\1/p' | head -n 1; }
is_word() {
  local LC_ALL=C
  case "$1" in *[!A-Za-z0-9._+-]* | "") return 1 ;; esac
  return 0
}
TC_BASE=$(tree_tc "$BASE_SHA") || TC_BASE=""
TC_WAVE=$(tree_tc "$WAVE_SHA") || TC_WAVE=""
tc_word() { if is_word "$1"; then printf '%s' "$1"; else printf 'unreadable'; fi; }

# ---- 2. scratch ----------------------------------------------------------------
# The block needs stdout and the line after it stderr: a closed one stops the
# run here, before a scratch dir exists (bash checks /dev/fd/N by descriptor).
if ! [ -e /dev/fd/1 ] || ! [ -e /dev/fd/2 ]; then die_usage "stdout or stderr is closed"; fi
# mktemp's own error names the scratch root: it goes nowhere, the fixed text
# stands for it.
X_MADE=$(mktemp -d "$TMPROOT/consent-point.XXXXXX" 2>/dev/null) || die_usage "mktemp failed under the scratch root"
# Until the run's own EXIT trap is set, a stop removes the new, still empty
# scratch dir.
# shellcheck disable=SC2329  # invoked by the EXIT trap
early_exit() {
  case "$X_MADE" in "$TMPROOT"/consent-point.?*) rm -rf -- "$X_MADE" 2>/dev/null || true ;; esac
}
trap early_exit EXIT
X=$(canon "$X_MADE") || die_usage "the scratch dir cannot be entered"
LOGS=$X/logs
LISTS=$X/lists
RUN=$X/run
SNAP=$X/snap
B=$X/bin
BUILD=$X/build
T=$BUILD/target
mkdir -p "$LOGS" "$LISTS" "$RUN" "$SNAP" "$B" "$BUILD/base" "$BUILD/wave" 2>/dev/null || die_usage "the scratch dir cannot be written"
BODY=$X/paste.body

# fd 3 is the terminal's stdout, for the block alone; fd 4 its stderr, for the
# line after the block and a one-line pre-flight message alone. Every child
# that runs code other than the shell's own text tools gets both closed
# (`3>&- 4>&-`).
exec 3>&1 4>&2
exec 1>>"$LOGS/script.stdout" 2>>"$LOGS/script.stderr" </dev/null

FAILED=""     # space-separated step names
STEP="start"
BLOCK_DONE=0
START_SECONDS=$SECONDS

emit() { printf '%s\n' "$*" >>"$BODY"; }
fail() {
  case " $FAILED " in *" $1 "*) return 0 ;; esac
  FAILED="$FAILED $1"
  emit "step-failed $1"
  echo "step failed: $1" >&2
}
# A step's exit status as its failure's name: a tool killed at the time limit
# (SIGALRM, 142) is `<step>-timeout`, a signal from the terminal
# `<step>-interrupted`.
fail_rc() {   # step rc
  case "$2" in
    142) fail "$1-timeout" ;;
    129 | 130 | 143) fail "$1-interrupted" ;;
    *) fail "$1" ;;
  esac
}

# >>> block_filter (consent-point-test.sh defines this function from these lines)
# The block's alphabet, byte by byte (LC_ALL=C here as well as globally): a
# body line holding any byte outside [A-Za-z0-9 ._:=(),+-] — '/' among them, so
# no path, and every byte of a UTF-8 letter — is dropped and counted, never
# printed. A second net: every line the body holds is built from fixed words,
# commit ids, crate versions and numbers.
block_filter() {
  local LC_ALL=C line dropped=0
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      *[!A-Za-z0-9\ ._:=\(\),+-]*) dropped=$((dropped + 1)) ;;
      *) printf '%s\n' "$line" ;;
    esac
  done
  [ "$dropped" = 0 ] || echo "dropped-lines $dropped (outside the block's alphabet; nothing of them printed)"
}
# <<< block_filter

# Print the block to stdout: the frames, the filtered body, the status. Stdout
# holds nothing else, so `| pbcopy` or `> file` takes the block alone.
print_block() {
  local rc=$1 n
  [ "$BLOCK_DONE" = 0 ] || return 0
  BLOCK_DONE=1
  # shellcheck disable=SC2086  # word count of the step names
  n=$(set -- $FAILED; echo $#)
  {
    echo "=== PASTE FROM HERE ==="
    if [ -f "$BODY" ]; then block_filter <"$BODY"; fi
    if [ "$n" = 0 ]; then echo "status complete"; else echo "status incomplete steps-failed=$n"; fi
    echo "exit $rc"
    echo "=== TO HERE ==="
  } >&3 || true
}
# The line after the block, on stderr: where the scratch dir is (ASCII only,
# like the block).
print_trailer() {
  echo "($PROG, not for pasting: $X holds the index lists that map each index to its file, the tools' outputs, the logs, and snapshots with your file paths and the SHA-256 of every file the run read and of every file under config/ and plugins/; remove it when done.)" >&4 || true
}

# The build tree, removed whenever the run ends without --keep. After Ctrl-C a
# rustc of the interrupted build can still be writing into it for a moment, so
# the removal is tried again, a second apart, up to ten times.
cleanup_build() {
  local i
  [ "$KEEP" = 0 ] || return 0
  case "$BUILD" in "$X"/build) ;; *) return 0 ;; esac
  for i in 1 2 3 4 5 6 7 8 9 10; do
    [ -e "$BUILD" ] || return 0
    rm -rf -- "$BUILD" 2>>"$LOGS/cleanup.log" 3>&- 4>&- || true
    [ -e "$BUILD" ] || return 0
    echo "cleanup attempt $i left the build tree; trying again" >>"$LOGS/cleanup.log"
    sleep 1 3>&- 4>&- || true
  done
}
# The end of every run that reaches the scratch dir: no signal cuts the
# cleanup or the block short.
finish() {   # rc [one line for the terminal's stderr, after the block]
  trap '' INT TERM HUP
  cleanup_build
  print_block "$1"
  if [ $# -ge 2 ]; then echo "$2" >&4 || true; fi
  print_trailer
  exit "$1"
}

# shellcheck disable=SC2329  # invoked by the EXIT trap
on_exit() {
  local rc=$?
  trap '' INT TERM HUP
  if [ "$BLOCK_DONE" = 0 ]; then
    # An unexpected stop: name the step, never the error.
    [ "$rc" != 0 ] || rc=1
    case "$rc" in
      129 | 130 | 143) fail "$STEP-interrupted" ;;
      *) fail "$STEP-unexpected" ;;
    esac
    cleanup_build
    print_block "$rc"
    print_trailer
  fi
  exit "$rc"
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

emit "consent-point v2"
emit "base $BASE_SHA toolchain $(tc_word "$TC_BASE")"
emit "wave $WAVE_SHA toolchain $(tc_word "$TC_WAVE")"
emit "options include-checkout-config=$([ "$INCLUDE_CHECKOUT" = 1 ] && echo yes || echo no) keep-build=$([ "$KEEP" = 1 ] && echo yes || echo no) tool-time-limit=$TOOL_LIMIT"

# ---- 1b. perl and the toolchains (never installed here) --------------------------
STEP=toolchain-check
if ! command -v perl >/dev/null 2>&1; then
  fail perl-check
  emit "perl-missing"
  finish 1 "$PROG: perl must be on PATH (it bounds each tool's run); nothing was built or read."
fi
# rustup (1.29) installs the active toolchain on `rustup toolchain list` when
# it is missing; the active one comes from the cwd's rust-toolchain.toml (the
# checkout's working tree, which may pin a toolchain not yet installed) or from
# RUSTUP_TOOLCHAIN. Run from /, with RUSTUP_AUTO_INSTALL=0 and no
# RUSTUP_TOOLCHAIN, the check only lists.
tc_installed() {
  (cd / && exec env -u RUSTUP_TOOLCHAIN RUSTUP_AUTO_INSTALL=0 rustup toolchain list) 2>/dev/null </dev/null 3>&- 4>&- |
    awk -v t="$1" '{ if ($1 == t || index($1, t "-") == 1) f = 1 } END { exit !f }'
}
TC_MISSING=""
if ! command -v rustup >/dev/null 2>&1 || ! command -v cargo >/dev/null 2>&1; then
  fail toolchain-check
  emit "toolchain-missing rustup-or-cargo"
  finish 1 "$PROG: rustup and cargo must be on PATH; nothing was built or read."
fi
for tc in "$TC_BASE" "$TC_WAVE"; do
  if ! is_word "$tc"; then
    fail toolchain-read
    continue
  fi
  if ! tc_installed "$tc"; then
    case " $TC_MISSING " in *" $tc "*) ;; *) TC_MISSING="$TC_MISSING $tc" ;; esac
  fi
done
if [ -n "$TC_MISSING" ]; then
  fail toolchain-check
  emit "toolchain-missing$TC_MISSING"
  finish 1 "$PROG: toolchain(s)$TC_MISSING not installed; install with 'rustup toolchain install <name>' and run again (nothing was built or read)."
fi
if [ -n "$FAILED" ]; then
  finish 1 "$PROG: a tree's rust-toolchain.toml has no channel line this script reads; nothing was built or read."
fi

# ---- 6a. the store's snapshot, before anything else reads or builds -------------
# Every change to a file's bytes or metadata moves its ctime (an xattr, a flag,
# an ACL, a mode); reading it never does. %f is the BSD flags, %i the inode.
if [ "$(uname -s)" = Darwin ]; then
  ST_ENTRY=(-f '%N %HT %Lp %m %c %f %i')         # name, type, mode, mtime, ctime, flags, inode
  ST_LINK=(-f '%N -> %Y')                        # a link and its target
  ST_THROUGH=(-L -f '%HT %Lp %m %c %f %i %z')    # through links: the same, and the size
else
  ST_ENTRY=(-c '%n %F %a %Y %Z %i')
  ST_LINK=(-c '%N')
  ST_THROUGH=(-L -c '%F %a %Y %Z %i %s')
fi
# README's `snap`, over config/ and plugins/: every entry with its type, mode,
# times, flags and inode, every regular file's SHA-256, every link's target,
# the entry count. It stays in the scratch dir.
snap() {
  local name d
  for name in config plugins; do
    d=$HOMEC/$name
    if [ ! -e "$d" ] && [ ! -L "$d" ]; then
      echo "absent $name"
      continue
    fi
    echo "present $name"
    (
      cd "$d" 2>/dev/null || { echo "unenterable"; exit 0; }
      # A directory find cannot read is an error line, kept in the snapshot
      # (the same before and after), never a stop.
      { find . -exec stat "${ST_ENTRY[@]}" {} + 2>&1 || true; } | LC_ALL=C sort
      { find . -type f -exec shasum -a 256 {} + 2>&1 || true; } | LC_ALL=C sort
      { find . -type l -exec stat "${ST_LINK[@]}" {} + 2>&1 || true; } | LC_ALL=C sort
      { find . 2>/dev/null || true; } | wc -l
    ) 3>&- 4>&- || echo "snapshot-error $name"
  done
}
STEP=snapshot-before
snap >"$SNAP/before.txt" 2>>"$LOGS/snap.log" || fail snapshot-before

# ---- 3. exports -----------------------------------------------------------------
STEP="export"
EXPORT_OK=1
if ! git -C "$R" archive --format=tar "$BASE_SHA" 2>>"$LOGS/export.log" 3>&- 4>&- |
  tar -x -f - -C "$BUILD/base" 2>>"$LOGS/export.log" 3>&- 4>&-; then
  fail export-base
  EXPORT_OK=0
fi
if ! git -C "$R" archive --format=tar "$WAVE_SHA" 2>>"$LOGS/export.log" 3>&- 4>&- |
  tar -x -f - -C "$BUILD/wave" 2>>"$LOGS/export.log" 3>&- 4>&-; then
  fail export-wave
  EXPORT_OK=0
fi
E=$BUILD/base/tasks/deps-upgrade/tools/e6
if [ ! -d "$E/typedprobe" ]; then
  fail export-base-has-no-e6-tools
  EXPORT_OK=0
fi

# yamlcmp's SkillFrontmatter, taken from the base tree. The tool's committed
# copy (yamlcmp/src/types.rs) is taken by hand and can fall behind its source:
# it missed N-SKILL-2's `#[serde(flatten)] unknown_keys` (8c63804f), which
# changes how serde reads every frontmatter (a map read with unknown values
# buffered), so a stale copy gives wrong words on the typed facet. The run
# builds yamlcmp on the base's own struct instead, with the copy's two
# documented changes — `impl ScriptConfig` left out (it needs openalpaca_llm),
# `validate()` returning a String — each anchor matched exactly once and no
# reference into the crate left, or the step fails as `yamlcmp-struct-derive`.
# `committed-copy-matches-base` (informational) says whether the committed copy,
# its head comment aside, is still byte for byte that struct; the README's
# by-hand Build block uses the committed copy.
# shellcheck disable=SC2016  # perl code, expanded by perl
DERIVE_PL='
  my $f = shift @ARGV;
  open(my $h, "<", $f) or exit 3;
  local $/;
  my $s = <$h>;
  close $h;
  my $n = () = $s =~ /^impl ScriptConfig \{\n/mg;
  exit 4 unless $n == 1;
  $s =~ s/^impl ScriptConfig \{\n.*?^\}\n\n?//ms or exit 4;
  my $sig = "    pub(super) fn validate(&self) -> Result<(), super::SkillParseError> {\n";
  $n = () = $s =~ /^\Q$sig\E/mg;
  exit 5 unless $n == 1;
  $s =~ s/^\Q$sig\E/    pub fn validate(&self) -> Result<(), String> {\n/m or exit 5;
  $n = ($s =~ s/Err\(super::SkillParseError::MissingField\("([a-z_]+)"\)\)/Err("missing $1".to_string())/g);
  exit 6 unless $n >= 1;
  exit 7 if $s =~ /super::|crate::|openalpaca_/;
  print $s;
'
STEP=yamlcmp-struct
YAML_STRUCT_OK=0
YS_SRC=$BUILD/base/crates/openalpaca_core/src/middleware/skill/types.rs
YS_DST=$E/yamlcmp/src/types.rs
if [ "$EXPORT_OK" = 1 ]; then
  YS_MATCH=no
  if [ -f "$YS_SRC" ] && [ -f "$YS_DST" ] &&
    perl -e "$DERIVE_PL" "$YS_SRC" >"$LOGS/yamlcmp-types.derived" 2>>"$LOGS/yamlcmp-struct.log" </dev/null 3>&- 4>&- &&
    awk 'h == 0 && /^\/\// { next } { h = 1; print }' "$YS_DST" >"$LOGS/yamlcmp-types.committed" &&
    awk 'h == 0 && /^\/\// { print; next } { h = 1 }' "$YS_DST" >"$LOGS/yamlcmp-types.head"; then
    if cmp -s "$LOGS/yamlcmp-types.derived" "$LOGS/yamlcmp-types.committed" 3>&- 4>&-; then YS_MATCH=yes; fi
    case "$YS_DST" in
      "$X"/build/base/*) cat "$LOGS/yamlcmp-types.head" "$LOGS/yamlcmp-types.derived" >"$YS_DST" && YAML_STRUCT_OK=1 ;;
    esac
  fi
  if [ "$YAML_STRUCT_OK" = 1 ]; then
    emit "yamlcmp-struct source=base committed-copy-matches-base $YS_MATCH"
  else
    fail yamlcmp-struct-derive
  fi
fi

# Which probed loaders --base changed since it and --wave parted: typedprobe-
# wave runs the wave's own copy of each, so a `tp-<kind>` disagreement is the
# crate bump's alone only for a kind marked `no`. A kind is `yes` when any file
# under its paths other than a test file changed between the merge base and
# --base (`git diff --name-only` between two commits reads objects only).
STEP=wave-loader
loader_paths() {
  case "$1" in
    skill) echo crates/openalpaca_core/src/middleware/skill ;;
    persona) echo crates/openalpaca_core/src/middleware/soul crates/openalpaca_core/src/middleware/user \
      crates/openalpaca_core/src/middleware/identity crates/openalpaca_core/src/middleware/bootstrap \
      crates/openalpaca_core/src/middleware/persona_frontmatter.rs crates/openalpaca_core/src/middleware/body_sections.rs ;;
    agent) echo crates/openalpaca_core/src/agent ;;
    daemon-config) echo crates/openalpaca_core/src/daemon_config ;;
    mcp-config) echo crates/openalpaca_core/src/tools/mcp/config.rs ;;
    llm-config) echo crates/openalpaca_llm/src/config/llm_config ;;
    plugin-manifest) echo crates/openalpaca_plugins/src/manifest.rs crates/openalpaca_plugins/src/manifest ;;
  esac
}
LOADER_KINDS="skill persona agent daemon-config mcp-config llm-config plugin-manifest"
WL_PATHS=()
for k in $LOADER_KINDS; do
  # shellcheck disable=SC2046  # fixed paths, no spaces
  for p in $(loader_paths "$k"); do WL_PATHS+=("$p"); done
done
WL_MB=$(git -C "$R" merge-base "$BASE_SHA" "$WAVE_SHA" 2>>"$LOGS/wave-loader.log" </dev/null 3>&- 4>&-) || WL_MB=""
if is_sha "$WL_MB" &&
  GIT_OPTIONAL_LOCKS=0 git -C "$R" diff --name-only --no-renames "$WL_MB" "$BASE_SHA" -- "${WL_PATHS[@]}" \
    >"$LOGS/wave-loader.files" 2>>"$LOGS/wave-loader.log" </dev/null 3>&- 4>&-; then
  grep -v -E '(^|/)tests/|(^|/)([A-Za-z0-9_]+_)?tests\.rs$' "$LOGS/wave-loader.files" >"$LOGS/wave-loader.nontest" || true
  line="wave-loader-behind-base files=$(wc -l <"$LOGS/wave-loader.nontest" | tr -d ' ')"
  for k in $LOADER_KINDS; do
    hit=no
    for p in $(loader_paths "$k"); do
      if awk -v p="$p" 'index($0, p "/") == 1 || $0 == p { f = 1 } END { exit !f }' "$LOGS/wave-loader.nontest"; then hit=yes; fi
    done
    line="$line $k=$hit"
  done
  emit "$line"
else
  emit "wave-loader-behind-base unknown"
fi

# ---- 4. the input lists ---------------------------------------------------------
STEP=lists
# The lists: toml (tomlcmp, tedit), llm (keycount, typedprobe llm-edit),
# plugcfg (keycount), skills (yamlcmp, typedprobe skill), and one per
# remaining typedprobe kind. `<list>.paths` holds one path per line (the
# arguments); `<list>.list` is `<index>\t<source>\t<path>` for the owner.
ALL_LISTS="toml llm plugcfg skills tp-daemon tp-mcp tp-agent-toml tp-plugin tp-agent-md tp-soul tp-user tp-identity tp-bootstrap"
CONFIG_LISTS="toml llm tp-daemon tp-mcp tp-agent-toml tp-agent-md skills tp-soul tp-user tp-identity tp-bootstrap"
for l in $ALL_LISTS; do : >"$LISTS/$l.paths"; : >"$LISTS/$l.sources"; done
: >"$LISTS/missing.tally"
: >"$LISTS/skipped.tally"
SRC=home          # the source being collected: home, extra, checkout, checkout-local
COUNT_MISSING=1
COUNT_ONLY=0      # checkout-local without the flag: count the files, feed nothing
LOCAL_COUNT=0     # checkout-local files the export does not hold byte for byte
SKILL_NOT_SCANNED=0
PLUGIN_SKILL_MD=0

note_missing() { [ "$COUNT_MISSING" = 0 ] || printf '%s\n' "$@" >>"$LISTS/missing.tally"; }
note_skipped() { [ "$COUNT_ONLY" = 1 ] || printf '%s\n' "$@" >>"$LISTS/skipped.tally"; }
listable() { [ -d "$1" ] && [ -r "$1" ] && [ -x "$1" ]; }

# Add one path to one or more lists, when it is a regular file (a link to one
# is followed, as the daemon's reads follow it); a FIFO, a device, a directory
# or a broken link is skipped and counted, never handed to a tool (a FIFO would
# block its read). From the checkout's working tree (checkout-local), a file the
# base export holds byte for byte is already fed from the export and is passed
# over; the rest is counted, and fed only under --include-checkout-config.
add() {   # path list...
  local f=$1 rel ex
  shift
  if [ "$SRC" = checkout-local ]; then
    rel=${f#"$R"/config/}
    ex=$BUILD/base/config/$rel
    if [ -f "$ex" ] && [ -f "$f" ] && cmp -s "$f" "$ex" 2>/dev/null 3>&- 4>&-; then return 0; fi
    LOCAL_COUNT=$((LOCAL_COUNT + 1))
    [ "$COUNT_ONLY" = 0 ] || return 0
  fi
  if has_nl "$f"; then
    note_skipped "$@"
    return 0
  fi
  if [ -f "$f" ]; then
    local l
    for l in "$@"; do
      printf '%s\n' "$f" >>"$LISTS/$l.paths"
      printf '%s\n' "$SRC" >>"$LISTS/$l.sources"
    done
  else
    note_skipped "$@"
  fi
}
# A file the daemon looks for by name: absent is `missing`, anything but a
# regular file `skipped`.
add_named() {   # path list...
  local f=$1
  if [ ! -e "$f" ] && [ ! -L "$f" ]; then
    shift
    note_missing "$@"
    return 0
  fi
  add "$@"
}
# The entries of a directory, NUL-separated and LC_ALL=C sorted; the caller
# has checked that it can be listed.
entries() {   # dir find-args...
  local d=$1
  shift
  { find -H "$d" -mindepth 1 -maxdepth 1 ${1+"$@"} -print0 2>>"$LOGS/find.log" 3>&- 4>&- || true; } | LC_ALL=C sort -z
}
# How many entries a find prints, counted by their NULs (a name may hold a
# line break).
count_found() {   # find-args...
  { find ${1+"$@"} -print0 2>>"$LOGS/find.log" 3>&- 4>&- || true; } | LC_ALL=C tr -cd '\000' | wc -c | tr -d ' '
}

# A config directory, the way the daemon reads one (resolve_config_base_dir's
# result): llm.toml, daemon.toml, mcp.toml by name; tools/*.toml
# (services/tools.rs); agents/*.toml and agents/*.md, a flat read_dir by
# extension (services/agents.rs); skills/<child>/SKILL.md for each direct
# child directory (services/mod.rs → SkillCatalog::scan_directory, one level,
# no deeper walk: `is_dir()` then `join("SKILL.md").exists()`, so a child it
# cannot enter is passed over by the daemon in silence and counted `skipped`
# here); orchestrator/{SOUL,USER,IDENTITY,BOOTSTRAP}.md (bootstrap/persona.rs).
collect_config() {   # dir
  local d=$1 f child n_all n_two
  if [ -d "$d" ] && [ ! -x "$d" ]; then
    # shellcheck disable=SC2086  # the list names
    note_skipped $CONFIG_LISTS
    return 0
  fi
  add_named "$d/llm.toml" toml llm
  add_named "$d/daemon.toml" toml tp-daemon
  add_named "$d/mcp.toml" toml tp-mcp
  if [ ! -d "$d/tools" ]; then
    note_missing toml
  elif ! listable "$d/tools"; then
    note_skipped toml
  else
    while IFS= read -r -d '' f; do add "$f" toml; done < <(entries "$d/tools" -name '*.toml')
  fi
  if [ ! -d "$d/agents" ]; then
    note_missing tp-agent-md
  elif ! listable "$d/agents"; then
    note_skipped toml tp-agent-toml tp-agent-md
  else
    while IFS= read -r -d '' f; do add "$f" toml tp-agent-toml; done < <(entries "$d/agents" -name '*.toml')
    while IFS= read -r -d '' f; do add "$f" tp-agent-md; done < <(entries "$d/agents" -name '*.md')
  fi
  if [ ! -d "$d/skills" ]; then
    note_missing skills
  elif ! listable "$d/skills"; then
    note_skipped skills
  else
    while IFS= read -r -d '' child; do
      [ -d "$child" ] || continue                                        # the scan skips a non-directory
      [ -x "$child" ] || { note_skipped skills; continue; }               # one it cannot enter: counted
      [ -e "$child/SKILL.md" ] || [ -L "$child/SKILL.md" ] || continue    # and a directory with no SKILL.md
      add "$child/SKILL.md" skills
    done < <(entries "$d/skills")
    if [ "$SRC" != checkout-local ]; then
      # A SKILL.md the one-level scan never loads (flat in skills/, or deeper
      # than skills/<child>/): counted, not fed. Neither find follows a link
      # below skills/, so the two counts are taken over the same entries.
      n_all=$(count_found -H "$d/skills" -mindepth 1 -name SKILL.md)
      n_two=$(count_found -H "$d/skills" -mindepth 2 -maxdepth 2 -name SKILL.md)
      SKILL_NOT_SCANNED=$((SKILL_NOT_SCANNED + n_all - n_two))
    fi
  fi
  if [ -d "$d/orchestrator" ] && [ ! -x "$d/orchestrator" ]; then
    note_skipped tp-soul tp-user tp-identity tp-bootstrap
  else
    add_named "$d/orchestrator/SOUL.md" tp-soul
    add_named "$d/orchestrator/USER.md" tp-user
    add_named "$d/orchestrator/IDENTITY.md" tp-identity
    add_named "$d/orchestrator/BOOTSTRAP.md" tp-bootstrap   # deleted once onboarding ends: missing is normal
  fi
}
# The plugins directory: .permissions.toml, .config/*.toml (the DU-10 count's
# plugin half), and <child>/plugin.toml for each direct child directory
# holding one (PluginManager::plugin_directories: `is_dir()` then
# `join("plugin.toml").exists()`; a child it cannot enter is counted `skipped`).
collect_plugins() {   # dir
  local d=$1 f child
  if [ ! -d "$d" ]; then
    note_missing plugcfg tp-plugin
    return 0
  fi
  if [ ! -x "$d" ]; then
    note_skipped toml plugcfg tp-plugin
    return 0
  fi
  add_named "$d/.permissions.toml" toml plugcfg
  if [ -d "$d/.config" ]; then
    if listable "$d/.config"; then
      while IFS= read -r -d '' f; do add "$f" toml plugcfg; done < <(entries "$d/.config" -name '*.toml')
    else
      note_skipped toml plugcfg
    fi
  fi
  if ! listable "$d"; then
    note_skipped toml tp-plugin
    return 0
  fi
  while IFS= read -r -d '' child; do
    [ -d "$child" ] || continue
    [ -x "$child" ] || { note_skipped toml tp-plugin; continue; }
    [ -e "$child/plugin.toml" ] || [ -L "$child/plugin.toml" ] || continue
    add "$child/plugin.toml" toml tp-plugin
    # A plugin's skills come from its process (`skill/info`), never from a
    # SKILL.md file; any such file is counted here and not fed (README's
    # "plugin-contributed ones" read against the code).
    case "${child##*/}" in .*) continue ;; esac
    PLUGIN_SKILL_MD=$((PLUGIN_SKILL_MD + $(count_found "$child" -name SKILL.md)))
  done < <(entries "$d")
}

# The sources, in this fixed order.
SRC=home COUNT_MISSING=1
collect_config "$HOMEC/config"
collect_plugins "$HOMEC/plugins"

HOME_CFG_C=$(canon "$HOMEC/config" || true)
CHECKOUT_CFG_C=$(canon "$R/config" || true)
EXTRA_STATE="unset"
if [ -n "${OPENALPACA_CONFIG_DIR:-}" ]; then
  if [ -d "$OPENALPACA_CONFIG_DIR" ]; then
    EXTRA_C=$(canon "$OPENALPACA_CONFIG_DIR" || true)
    if [ -z "$EXTRA_C" ] || has_nl "$EXTRA_C"; then
      EXTRA_STATE="cannot-be-entered"
    elif [ "$EXTRA_C" = "$HOME_CFG_C" ]; then
      EXTRA_STATE="same-as-home"
    elif [ "$EXTRA_C" = "$CHECKOUT_CFG_C" ]; then
      EXTRA_STATE="same-as-checkout"   # the checkout passes below read it
    elif [ "$INCLUDE_CHECKOUT" = 1 ]; then
      EXTRA_STATE="included"
      SRC=extra COUNT_MISSING=1
      collect_config "$EXTRA_C"
    else
      EXTRA_STATE="not-included"
      if [ -e "$EXTRA_C/llm.toml" ] || [ -L "$EXTRA_C/llm.toml" ]; then
        EXTRA_STATE="not-included-has-llm"
      fi
    fi
  else
    EXTRA_STATE="not-a-directory"
  fi
fi

# The checkout's tracked config, from the base export (byte for byte the
# committed files of --base).
SRC=checkout COUNT_MISSING=0
collect_config "$BUILD/base/config"

# What a daemon started from the checkout reads there beyond the export (its
# walk-up finds ./config by config/llm.toml): every file of a loaded kind in the
# working tree's config/ whose bytes the export does not hold — fed only with
# the flag; without it counted (a file the export lacks is only stat'ed).
CHECKOUT_LLM=no
if [ -e "$R/config/llm.toml" ] || [ -L "$R/config/llm.toml" ]; then CHECKOUT_LLM=yes; fi
SRC=checkout-local COUNT_MISSING=0
if [ "$INCLUDE_CHECKOUT" = 1 ]; then COUNT_ONLY=0; else COUNT_ONLY=1; fi
if [ -d "$R/config" ]; then collect_config "$R/config"; fi
COUNT_ONLY=0
emit "checkout-llm-toml present=$CHECKOUT_LLM included=$([ "$INCLUDE_CHECKOUT" = 1 ] && [ "$CHECKOUT_LLM" = yes ] && echo yes || echo no)"
emit "checkout-config-local files=$LOCAL_COUNT included=$([ "$INCLUDE_CHECKOUT" = 1 ] && echo yes || echo no)"
emit "openalpaca-config-dir $EXTRA_STATE"

# Write the owner's index lists and the per-list counts.
count_in() { grep -c -x -F "$1" "$2" 2>/dev/null || true; }
for l in $ALL_LISTS; do
  paste "$LISTS/$l.sources" "$LISTS/$l.paths" | awk 'BEGIN{OFS="\t"} {print NR, $0}' >"$LISTS/$l.list"
  n=$(wc -l <"$LISTS/$l.paths" | tr -d ' ')
  emit "list $l inputs=$n home=$(count_in home "$LISTS/$l.sources") extra=$(count_in extra "$LISTS/$l.sources") checkout=$(count_in checkout "$LISTS/$l.sources") checkout-local=$(count_in checkout-local "$LISTS/$l.sources") missing=$(count_in "$l" "$LISTS/missing.tally") skipped=$(count_in "$l" "$LISTS/skipped.tally")"
done
emit "skill-md-not-loaded-by-scan $SKILL_NOT_SCANNED (not fed)"
emit "skill-md-under-plugin-dirs $PLUGIN_SKILL_MD (not loaded by the daemon, not fed)"

# Read a list into the array named LIST_ARR.
load_list() {
  LIST_ARR=()
  local line
  while IFS= read -r line; do LIST_ARR+=("$line"); done <"$LISTS/$1.paths"
}
list_len() { wc -l <"$LISTS/$1.paths" | tr -d ' '; }

# ---- 6b. the fed files' snapshot, before any build ------------------------------
# Every file fed to a tool, read through links wherever it lives: its type,
# mode, times, flags, inode and size, and its SHA-256.
inputs_state() {
  local f
  LC_ALL=C sort -u "$LISTS"/*.paths | while IFS= read -r f; do
    printf '%s\n' "$f"
    if [ -f "$f" ]; then
      stat "${ST_THROUGH[@]}" "$f" 2>&1 || true
      shasum -a 256 "$f" 2>&1 || true
    else
      echo "not-a-regular-file"
    fi
  done
}
STEP=snapshot-inputs-before
inputs_state >"$SNAP/inputs.before" 2>>"$LOGS/snap.log" 3>&- 4>&- || fail snapshot-inputs-before

# ---- 5. builds ------------------------------------------------------------------
BUILD_START=$SECONDS
# README's `c()`: the toolchain named, the build dir pinned to the target dir
# (MF-5), run from inside the tree being built so that tree's .cargo/config.toml
# is the one cargo reads (the env vars override its build-dir and colour). An
# empty RUSTC_WRAPPER / RUSTC_WORKSPACE_WRAPPER clears a wrapper set in any
# cargo config file (cargo's documented reset), so no sccache-like tool writes
# a cache outside the scratch dir or starts a server; MAKEFLAGS and its kin are
# unset, so cargo never takes a jobserver from inherited descriptors.
cbuild() {
  local step=$1 tc=$2 cwd=$3 m=$4
  shift 4
  (cd "$cwd" && exec env -u CARGO_BUILD_TARGET -u E6_SELFTEST_PANIC -u MAKEFLAGS -u MFLAGS -u CARGO_MAKEFLAGS \
    RUSTUP_AUTO_INSTALL=0 CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0 \
    RUSTC_WRAPPER='' RUSTC_WORKSPACE_WRAPPER='' \
    CARGO_TARGET_DIR="$T" CARGO_BUILD_BUILD_DIR="$T" \
    cargo "+$tc" build --manifest-path "$m" ${1+"$@"}) >>"$LOGS/$step.log" 2>&1 </dev/null 3>&- 4>&-
}
# Every version of one crate a lock holds, comma-joined (the block's evidence
# that each tool was built on its pinned versions).
lockver() {
  local v
  v=$(awk -v c="$2" '/^name = /{ n = $3; gsub(/"/, "", n) } /^version = /{ if (n == c) { v = $3; gsub(/"/, "", v); print v } }' "$1" 2>/dev/null |
    LC_ALL=C sort -u | paste -s -d , - 2>/dev/null) || v=""
  echo "${v:-none}"
}
pkgs() { awk '/^name = /{n=$3} /^version = /{print n, $3}' "$1" | tr -d '"' | LC_ALL=C sort -u; }
sed_esc() { printf '%s' "$1" | sed -e 's/[\\|&]/\\&/g'; }

TOOLS_OK=""
build_tool() {   # name manifest-subpath binary-name-in-bin
  local rc=0
  STEP=build-$1
  if [ "$EXPORT_OK" = 0 ]; then
    fail "build-$1"
    return 0
  fi
  cbuild "build-$1" "$TC_BASE" "$BUILD/base" "$E/$2" --locked || rc=$?
  if [ "$rc" = 0 ] && cp "$T/debug/$4" "$B/$3"; then
    TOOLS_OK="$TOOLS_OK $3"
  else
    fail_rc "build-$1" "$rc"
  fi
}
build_tool tomlcmp tomlcmp/Cargo.toml tomlcmp tomlcmp
if [ "$YAML_STRUCT_OK" = 1 ]; then
  build_tool yamlcmp yamlcmp/Cargo.toml yamlcmp yamlcmp
else
  fail build-yamlcmp
fi
build_tool keycount keycount/Cargo.toml keycount keycount
build_tool tedit-old tedit/old/Cargo.toml tedit-old tedit
build_tool tedit-new tedit/new/Cargo.toml tedit-new tedit
emit "tool-locks tomlcmp-toml=$(lockver "$E/tomlcmp/Cargo.lock" toml) tedit-old-toml_edit=$(lockver "$E/tedit/old/Cargo.lock" toml_edit) tedit-new-toml_edit=$(lockver "$E/tedit/new/Cargo.lock" toml_edit) yamlcmp-serde_yaml=$(lockver "$E/yamlcmp/Cargo.lock" serde_yaml) yamlcmp-serde-saphyr=$(lockver "$E/yamlcmp/Cargo.lock" serde-saphyr) keycount-toml=$(lockver "$E/keycount/Cargo.lock" toml)"

# README's `tp()`: typedprobe from a scratch copy beside a copy of the tree's
# lock, its three path lines and its `toml` line taken from that tree; then
# `comm` prints `typedprobe 0.1.0` and nothing else when no version moved.
tp_build() {   # name tree toolchain
  local name=$1 w=$2 tc=$3 P=$BUILD/typedprobe-$1 tomlline moved rc=0
  STEP=build-typedprobe-$name
  if [ "$EXPORT_OK" = 0 ]; then
    fail "build-typedprobe-$name"
    return 0
  fi
  mkdir -p "$P"
  if ! { cp -R "$E/typedprobe/src" "$P/" && cp "$w/Cargo.lock" "$P/Cargo.lock"; } 2>>"$LOGS/build-typedprobe-$name.log"; then
    fail "build-typedprobe-$name"
    return 0
  fi
  if ! tomlline=$(grep -m1 -E '^toml = ' "$w/Cargo.toml"); then
    fail "build-typedprobe-$name-toml-line"
    return 0
  fi
  sed -e "s|\.\./\.\./\.\./\.\./\.\./crates/|$(sed_esc "$w")/crates/|" \
    -e "s|^toml = .*|$(sed_esc "$tomlline")|" "$E/typedprobe/Cargo.toml" >"$P/Cargo.toml"
  if [ "$(grep -c -F "$w/crates/" "$P/Cargo.toml")" != 3 ] || ! grep -q -x -F "$tomlline" "$P/Cargo.toml"; then
    fail "build-typedprobe-$name-manifest"
    return 0
  fi
  cbuild "build-typedprobe-$name" "$tc" "$w" "$P/Cargo.toml" || rc=$?
  if [ "$rc" != 0 ] || ! cp "$T/debug/typedprobe" "$B/typedprobe-$name"; then
    fail_rc "build-typedprobe-$name" "$rc"
    return 0
  fi
  LC_ALL=C comm -23 <(pkgs "$P/Cargo.lock") <(pkgs "$w/Cargo.lock") >"$LOGS/typedprobe-$name.comm" 2>>"$LOGS/build-typedprobe-$name.log" || true
  moved=$(grep -c -v -x -F 'typedprobe 0.1.0' "$LOGS/typedprobe-$name.comm" || true)
  emit "typedprobe-$name-lock moved=$moved toml=$(lockver "$P/Cargo.lock" toml) toml_edit=$(lockver "$P/Cargo.lock" toml_edit) serde_yaml=$(lockver "$P/Cargo.lock" serde_yaml) serde-saphyr=$(lockver "$P/Cargo.lock" serde-saphyr) granit-parser=$(lockver "$P/Cargo.lock" granit-parser)"
  if [ "$moved" != 0 ]; then
    fail "typedprobe-$name-lock-moved"
    return 0
  fi
  TOOLS_OK="$TOOLS_OK typedprobe-$name"
}
tp_build base "$BUILD/base" "$TC_BASE"
tp_build wave "$BUILD/wave" "$TC_WAVE"
BUILD_SECONDS=$((SECONDS - BUILD_START))
emit "build seconds=$BUILD_SECONDS"
have() { case " $TOOLS_OK " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# The two trees against the comparators' pins. typedprobe-base must run the
# old sides' versions (keycount's toml, tedit-old's toml_edit, yamlcmp's
# serde_yaml): a base past P04 answers another question, so it fails the run.
# typedprobe-wave against the new sides (tomlcmp's other toml, tedit-new's
# toml_edit, yamlcmp's serde-saphyr) is informational: the default --wave
# holds serde-saphyr 1.3.0, and on the YAML axis yamlcmp is the word of record.
STEP=pin-check
yn() { if [ "$1" = "$2" ]; then echo yes; else echo no; fi; }
set_minus() {   # comma-list comma-list: the first's members not in the second
  local out="" v
  local IFS=,
  for v in $1; do
    case ",$2," in *",$v,"*) ;; *) out="$out${out:+,}$v" ;; esac
  done
  echo "${out:-none}"
}
if have typedprobe-base; then
  PL=$BUILD/typedprobe-base/Cargo.lock
  p1=$(yn "$(lockver "$PL" toml)" "$(lockver "$E/keycount/Cargo.lock" toml)")
  p2=$(yn "$(lockver "$PL" toml_edit)" "$(lockver "$E/tedit/old/Cargo.lock" toml_edit)")
  p3=$(yn "$(lockver "$PL" serde_yaml)" "$(lockver "$E/yamlcmp/Cargo.lock" serde_yaml)")
  emit "base-matches-old-pins toml=$p1 toml_edit=$p2 serde_yaml=$p3"
  [ "$p1$p2$p3" = yesyesyes ] || fail base-past-old-pins
fi
if have typedprobe-wave; then
  PL=$BUILD/typedprobe-wave/Cargo.lock
  p1=$(yn "$(lockver "$PL" toml)" "$(set_minus "$(lockver "$E/tomlcmp/Cargo.lock" toml)" "$(lockver "$E/keycount/Cargo.lock" toml)")")
  p2=$(yn "$(lockver "$PL" toml_edit)" "$(lockver "$E/tedit/new/Cargo.lock" toml_edit)")
  p3=$(yn "$(lockver "$PL" serde-saphyr)" "$(lockver "$E/yamlcmp/Cargo.lock" serde-saphyr)")
  emit "wave-matches-new-pins toml=$p1 toml_edit=$p2 serde-saphyr=$p3 (informational, on the YAML axis yamlcmp is the word of record)"
fi
# The crates under the old sides' toml, toml_edit (and the TOML stack they
# share) against typedprobe-base's lock, per old-side tool: `yes` when every
# version the tool's lock holds is one the base holds. Informational: one
# tomlcmp binary holds a single toml_parser 1.x for both of its sides, and the
# cut's toml_parser 1.1.5, toml_writer 1.1.3, toml_datetime 1.1.2,
# serde_spanned 1.1.2 and indexmap 2.14.2 differ from today's 1.1.3, 1.1.2,
# 1.1.1, 1.1.1 and 2.14.1 in no behaviour (their sources compared on
# 2026-10-27; README, *One command*).
if have typedprobe-base; then
  PL=$BUILD/typedprobe-base/Cargo.lock
  for tl in tomlcmp:tomlcmp/Cargo.lock tedit-old:tedit/old/Cargo.lock keycount:keycount/Cargo.lock; do
    line="old-side-stack-matches-base ${tl%%:*}"
    for c in toml_parser toml_writer toml_datetime serde_spanned indexmap winnow; do
      tv=$(lockver "$E/${tl#*:}" "$c")
      if [ "$tv" = none ]; then
        w=absent
      elif [ "$(set_minus "$tv" "$(lockver "$PL" "$c")")" = none ]; then
        w=yes
      else
        w=no
      fi
      line="$line $c=$w"
    done
    emit "$line (informational)"
  done
fi

# ---- 7. the runs ----------------------------------------------------------------
STEP=runs
: >"$LOGS/stderr-files.list"
# Run one tool from inside the scratch run dir, its stdout and stderr to files,
# under the time limit: perl sets the alarm and execs the tool in its place (a
# pending alarm survives exec), so the tool stays in the terminal's process
# group (Ctrl-C reaches it) and dies of SIGALRM at the limit.
# shellcheck disable=SC2016  # perl code, expanded by perl
LIMIT_PL='$SIG{ALRM} = "DEFAULT"; alarm shift @ARGV; exec { $ARGV[0] } @ARGV or exit 127'
run() {   # step out err cmd...
  local step=$1 out=$2 err=$3 rc=0
  shift 3
  STEP=$step
  printf '%s\n' "$err" >>"$LOGS/stderr-files.list"
  (cd "$RUN" && exec env -u E6_SELFTEST_PANIC -u RUST_BACKTRACE -u RUST_LIB_BACKTRACE \
    perl -e "$LIMIT_PL" "$TOOL_LIMIT" "$@") >"$out" 2>"$err" </dev/null 3>&- 4>&- || rc=$?
  [ "$rc" != 0 ] || return 0
  fail_rc "$step" "$rc"
  return 1
}

# Validate a tool's per-input output: exactly n lines, line i is `i\t<word>`
# with a word of the tool's own vocabulary. Prints the rows whose word is not
# in `clear` as `<label> <i> <word>`. A malformed output prints nothing of it.
WORDS_CMP="equal|different|parse-error-old|parse-error-new|parse-error-both|panic-old|panic-new|panic-both|unreadable"
WORDS_YAML="$WORDS_CMP|no-frontmatter"
WORDS_RT="roundtrip-ok|defect-shared|defect-old-only|defect-new-only|not-applicable|panic-old|panic-new|panic-both"
WORDS_TP="ok|err|panic"
tp_shape_ok() {   # file n
  awk -F '\t' -v n="$2" -v re="^($WORDS_TP)\$" '{ if (NF != 2 || $1 != NR || $2 !~ re) bad = 1 } END { exit (bad || NR != n) }' "$1"
}
rows() {   # label file n words clear-words
  local label=$1 file=$2 n=$3 words=$4 clear=$5
  if ! awk -F '\t' -v n="$n" -v re="^($words)\$" '
      { if (NF != 2 || $1 != NR || $2 !~ re) bad = 1 }
      END { exit (bad || NR != n) }' "$file"; then
    fail "output-shape-$label"
    return 1
  fi
  awk -F '\t' -v label="$label" -v re="^($clear)\$" '$2 !~ re { print label, $1, $2 }' "$file" >>"$BODY"
}
# Validate a `--summary` output: each line `<key>\t<number>`, the keys exactly
# the tool's own, in its order; emitted as one line `<label> key=n ...`.
summary() {   # label file keys...
  local label=$1 file=$2
  shift 2
  local want got
  want=$(printf '%s\n' "$@")
  got=$(awk -F '\t' '{ print $1 }' "$file")
  if [ "$want" != "$got" ] || ! awk -F '\t' '{ if (NF != 2 || $2 !~ /^[0-9]+$/) bad = 1 } END { exit bad }' "$file"; then
    fail "summary-shape-$label"
    return 1
  fi
  emit "$label $(awk -F '\t' '{ printf "%s%s=%s", (NR > 1 ? " " : ""), $1, $2 }' "$file")"
}
SUM_TOMLCMP=(inputs equal different parse-error-old parse-error-new parse-error-both panic-old panic-new panic-both unreadable facet:tree-different facet:to_string-different facet:to_string_pretty-different facet:leaf-display-different cross-read:new-reads-old-pretty-failed cross-read:old-reads-new-pretty-failed)
SUM_TEDIT=(inputs equal different parse-error-old parse-error-new parse-error-both panic-old panic-new panic-both unreadable typed-side-refused-old typed-side-refused-new scenarios-old scenarios-new roundtrip-defect-seeds-old roundtrip-defect-seeds-new roundtrip-defect-inputs-old roundtrip-defect-inputs-new roundtrip-defect-inputs-shared)
SUM_YAML=(inputs unreadable no-frontmatter typed:equal typed:different typed:parse-error-old typed:parse-error-new typed:parse-error-both typed:panic-old typed:panic-new typed:panic-both value:equal value:different value:parse-error-old value:parse-error-new value:parse-error-both value:panic-old value:panic-new value:panic-both)
SUM_KEYCOUNT=(inputs counted parse-error unreadable panic secret_ref use_keychain-true inputs-with-secret_ref inputs-with-use_keychain-true)

# tomlcmp and tedit over the TOML list.
N=$(list_len toml)
load_list toml
if [ "$N" = 0 ]; then
  emit "tomlcmp inputs=0 (not run)"
  emit "tedit inputs=0 (not run)"
else
  if have tomlcmp; then
    run run-tomlcmp "$RUN/tomlcmp.out" "$RUN/tomlcmp.err" "$B/tomlcmp" "${LIST_ARR[@]}" &&
      rows tomlcmp "$RUN/tomlcmp.out" "$N" "$WORDS_CMP" equal || true
    run run-tomlcmp-summary "$RUN/tomlcmp.summary" "$RUN/tomlcmp.summary.err" "$B/tomlcmp" --summary "${LIST_ARR[@]}" &&
      summary tomlcmp-summary "$RUN/tomlcmp.summary" "${SUM_TOMLCMP[@]}" || true
  else
    fail run-tomlcmp
  fi
  if have tedit-old && have tedit-new; then
    if run run-tedit-record "$RUN/tedit.record.out" "$RUN/tedit.record.err" "$B/tedit-old" record --out "$RUN/tedit.record" "${LIST_ARR[@]}"; then
      run run-tedit-compare "$RUN/tedit.out" "$RUN/tedit.err" "$B/tedit-new" compare --from "$RUN/tedit.record" "${LIST_ARR[@]}" &&
        rows tedit "$RUN/tedit.out" "$N" "$WORDS_CMP" equal || true
      run run-tedit-summary "$RUN/tedit.summary" "$RUN/tedit.summary.err" "$B/tedit-new" compare --from "$RUN/tedit.record" --summary "${LIST_ARR[@]}" &&
        summary tedit-summary "$RUN/tedit.summary" "${SUM_TEDIT[@]}" || true
      run run-tedit-roundtrip "$RUN/tedit.roundtrip" "$RUN/tedit.roundtrip.err" "$B/tedit-new" compare --from "$RUN/tedit.record" --roundtrip "${LIST_ARR[@]}" &&
        rows tedit-roundtrip "$RUN/tedit.roundtrip" "$N" "$WORDS_RT" "roundtrip-ok|defect-shared" || true
      if [ -s "$RUN/tedit.record.out" ]; then fail run-tedit-record-printed; fi
    fi
  else
    fail run-tedit
  fi
fi

# yamlcmp over the skill list: the typed facet (the word of record), its
# value facet, and the summary that counts both.
N=$(list_len skills)
load_list skills
if [ "$N" = 0 ]; then
  emit "yamlcmp inputs=0 (not run)"
elif have yamlcmp; then
  run run-yamlcmp "$RUN/yamlcmp.out" "$RUN/yamlcmp.err" "$B/yamlcmp" "${LIST_ARR[@]}" &&
    rows yamlcmp "$RUN/yamlcmp.out" "$N" "$WORDS_YAML" equal || true
  run run-yamlcmp-value "$RUN/yamlcmp-value.out" "$RUN/yamlcmp-value.err" "$B/yamlcmp" --facet value "${LIST_ARR[@]}" &&
    rows yamlcmp-value "$RUN/yamlcmp-value.out" "$N" "$WORDS_YAML" equal || true
  run run-yamlcmp-summary "$RUN/yamlcmp.summary" "$RUN/yamlcmp.summary.err" "$B/yamlcmp" --summary "${LIST_ARR[@]}" &&
    summary yamlcmp-summary "$RUN/yamlcmp.summary" "${SUM_YAML[@]}" || true
else
  fail run-yamlcmp
fi

# typedprobe: every kind under both builds (F-08), then base against wave.
tp_counts() { awk -F '\t' '{ c[$2]++ } END { printf "ok=%d err=%d panic=%d", c["ok"], c["err"], c["panic"] }' "$1"; }
for kl in daemon:tp-daemon llm-edit:llm mcp:tp-mcp agent-toml:tp-agent-toml plugin:tp-plugin skill:skills \
  agent-md:tp-agent-md soul:tp-soul user:tp-user identity:tp-identity bootstrap:tp-bootstrap; do
  kind=${kl%%:*}
  list=${kl##*:}
  N=$(list_len "$list")
  load_list "$list"
  if [ "$N" = 0 ]; then
    emit "tp-$kind inputs=0 (not run)"
    continue
  fi
  okv=""
  for v in base wave; do
    if have "typedprobe-$v"; then
      if run "run-tp-$v-$kind" "$RUN/tp-$v-$kind.out" "$RUN/tp-$v-$kind.err" "$B/typedprobe-$v" "$kind" "${LIST_ARR[@]}"; then
        if tp_shape_ok "$RUN/tp-$v-$kind.out" "$N"; then
          okv="$okv $v"
        else
          fail "output-shape-tp-$v-$kind"
        fi
      fi
    else
      fail "run-tp-$v-$kind"
    fi
  done
  line="tp-$kind inputs=$N"
  for v in base wave; do
    case " $okv " in
      *" $v "*) line="$line $v($(tp_counts "$RUN/tp-$v-$kind.out"))" ;;
      *) line="$line $v(not-run)" ;;
    esac
  done
  if [ "$okv" = " base wave" ]; then
    paste "$RUN/tp-base-$kind.out" "$RUN/tp-wave-$kind.out" >"$RUN/tp-$kind.pair"
    line="$line disagree=$(awk -F '\t' '$2 != $4 { d++ } END { print d + 0 }' "$RUN/tp-$kind.pair")"
    emit "$line"
    awk -F '\t' -v k="tp-$kind" '
      $2 != $4 { print k, $1, "base=" $2, "wave=" $4; next }
      $2 != "ok" { print k, $1, $2 }' "$RUN/tp-$kind.pair" >>"$BODY"
  else
    emit "$line"
    for v in $okv; do
      awk -F '\t' -v k="tp-$kind-$v" '$2 != "ok" { print k, $1, $2 }' "$RUN/tp-$v-$kind.out" >>"$BODY"
    done
  fi
done

# keycount — the DU-10 count over the llm.toml list and over the plugin list,
# and P12's waiver (README, *The DU-10 count*).
KC_LLM_STATE=not-run KC_PLUG_STATE=not-run
keycount_rows() {   # label file n
  if ! awk -F '\t' -v n="$3" '
      { if ($1 != NR) bad = 1
        else if (NF == 4 && $2 == "counted" && $3 ~ /^[0-9]+$/ && $4 ~ /^[0-9]+$/) {}
        else if (NF == 4 && $2 ~ /^(parse-error|unreadable|panic)$/ && $3 == "-" && $4 == "-") {}
        else bad = 1 }
      END { exit (bad || NR != n) }' "$2"; then
    fail "output-shape-$1"
    return 1
  fi
  awk -F '\t' -v k="$1" '
    $2 != "counted" { print k, $1, $2; next }
    $3 != 0 || $4 != 0 { print k, $1, "counted", "secret_ref=" $3, "use_keychain-true=" $4 }' "$2" >>"$BODY"
}
for kl in llm:keycount-llm plugcfg:keycount-plugins; do
  list=${kl%%:*}
  label=${kl##*:}
  N=$(list_len "$list")
  load_list "$list"
  if [ "$N" = 0 ]; then
    emit "$label inputs=0 (not run)"
    state=empty
  elif have keycount; then
    state=failed
    if run "run-$label" "$RUN/$label.out" "$RUN/$label.err" "$B/keycount" "${LIST_ARR[@]}" &&
      keycount_rows "$label" "$RUN/$label.out" "$N" &&
      run "run-$label-summary" "$RUN/$label.summary" "$RUN/$label.summary.err" "$B/keycount" --summary "${LIST_ARR[@]}" &&
      summary "$label-summary" "$RUN/$label.summary" "${SUM_KEYCOUNT[@]}"; then
      state=ran
    fi
  else
    fail "run-$label"
    state=failed
  fi
  if [ "$list" = llm ]; then KC_LLM_STATE=$state; else KC_PLUG_STATE=$state; fi
done
sumval() { awk -F '\t' -v k="$2" '$1 == k { print $2 }' "$1"; }
# P12's waiver of its keychain round trip holds only when every input of both
# runs reads `counted` and both summaries read `secret_ref 0` and
# `use_keychain-true 0` (no plugin file at all is a 0 for the plugin half),
# over an llm.toml list that holds every copy. The README leaves an empty
# llm.toml list open; here it does not hold (`no-llm-input`): a real store has
# one, so an empty list means the run did not see what P12 asks about. A file
# at a path either list reads that was not a regular file (or a directory the
# run could not list) was never counted: `skipped-input`.
summary_uncounted() { [ "$(sumval "$1" inputs)" != "$(sumval "$1" counted)" ]; }
summary_nonzero() { [ "$(sumval "$1" secret_ref)" != 0 ] || [ "$(sumval "$1" use_keychain-true)" != 0 ]; }
waiver() {
  local reason=""
  if [ "$KC_LLM_STATE" = failed ] || [ "$KC_PLUG_STATE" = failed ]; then
    reason=keycount-step-failed
  elif [ "$KC_LLM_STATE" = empty ]; then
    reason=no-llm-input
  elif [ "$(count_in llm "$LISTS/skipped.tally")" != 0 ] || [ "$(count_in plugcfg "$LISTS/skipped.tally")" != 0 ]; then
    reason=skipped-input
  else
    if summary_uncounted "$RUN/keycount-llm.summary"; then
      reason=uncounted-input
    elif [ "$KC_PLUG_STATE" = ran ] && summary_uncounted "$RUN/keycount-plugins.summary"; then
      reason=uncounted-input
    elif summary_nonzero "$RUN/keycount-llm.summary"; then
      reason=keychain-reference-counted
    elif [ "$KC_PLUG_STATE" = ran ] && summary_nonzero "$RUN/keycount-plugins.summary"; then
      reason=keychain-reference-counted
    fi
    if [ -z "$reason" ] && [ "$CHECKOUT_LLM" = yes ] && [ "$INCLUDE_CHECKOUT" = 0 ]; then
      reason=checkout-llm-not-counted
    fi
    if [ -z "$reason" ] && [ "$EXTRA_STATE" = not-included-has-llm ]; then
      reason=config-dir-llm-not-counted
    fi
  fi
  if [ -z "$reason" ]; then
    emit "P12-keychain-waiver holds (all-zero)"
  else
    emit "P12-keychain-waiver does-not-hold ($reason)"
  fi
}
STEP=waiver
waiver

STEP=snapshot-after
if snap >"$SNAP/after.txt" 2>>"$LOGS/snap.log" && [ -s "$SNAP/before.txt" ] && cmp -s "$SNAP/before.txt" "$SNAP/after.txt" 3>&- 4>&-; then
  emit "store-unchanged yes"
else
  emit "store-unchanged no"
fi
if inputs_state >"$SNAP/inputs.after" 2>>"$LOGS/snap.log" 3>&- 4>&- && cmp -s "$SNAP/inputs.before" "$SNAP/inputs.after" 3>&- 4>&-; then
  emit "inputs-unchanged yes"
else
  emit "inputs-unchanged no"
fi

# The tools' stderr: counted, never shown. A clean run has none; a panic
# outside a tool's catch prints only its fixed hook line (and exits 101).
STEP=stderr-count
total=0 hook=0
while IFS= read -r f; do
  [ -f "$f" ] || continue
  total=$((total + $(wc -l <"$f" | tr -d ' ')))
  hook=$((hook + $(grep -c -x -E '(tomlcmp|tedit|yamlcmp|typedprobe|keycount): a panic; its message is withheld because it could quote the input' "$f" || true)))
done <"$LOGS/stderr-files.list"
emit "tool-stderr lines=$total panic-hook=$hook other=$((total - hook))"
emit "elapsed seconds=$((SECONDS - START_SECONDS))"

# ---- the end --------------------------------------------------------------------
STEP="done"
RC=0
[ -z "$FAILED" ] || RC=1
finish "$RC"
