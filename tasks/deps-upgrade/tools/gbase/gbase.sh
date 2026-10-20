#!/usr/bin/env bash
# gbase.sh — the G-base driver: the dependency wave's re-cut, re-run at the branch point.
#
# plan-rev3.md §5.1 items 6, 6a and 6b (rev 3.9), §5.2's named gate G-base; amended for the plan
# review of 2026-10-17 (MF-1, MF-2, MF-5, MF-6, A1-17), then for the owner's D1 (a) and the
# cut-day compatibility report of 2026-10-17 (the defaults at the 2026-10-21T19:53Z instant, step
# 1b's reference re-derived, C5's bun.lock check), then for the owner's D4 (a) of 2026-10-19 (step
# 8 takes serde-saphyr 2.0, with the report's C1 and C10). From one canonical scratch root it exports
# the base commit and runs the re-cut in order — the toolchain step, the Tauri pair's Rust half
# (step 1b, P31a: the pair first, no hold), then the re-cut's steps 2-8 — each step its anchored
# manifest edits, its source hunks, its cargo commands, DU-23's freshness gate on what the step
# added or moved, a `cargo check` and a lock snapshot compared with the shipped one — then the
# full suite, the docs check, the P31a+P31+P32+P33 JS set with the new bun.lock dated against the
# cut, the four bun gates and G-GUI.
# It refuses to start before the cut instant (--not-before) and exits non-zero at the FIRST failed
# command; nothing is recorded-and-continued. Every log lands under $T/logs/, and a real run
# leaves $T/evidence-<date>-gbase.tar.gz of its logs and locks. README.md beside this file says
# how to read a run.
#
# Modes:  (default) run · --dry-run · --edits-only · --self-test     (see --help)
#
# Portable to bash 3.2 (macOS /bin/bash): no associative arrays, no mapfile.

set -Eeuo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)

# ---------------------------------------------------------------------------- arguments
BASE=""
TOOLCHAIN="1.99.0"
BUN=""
STEPS="all"
STRICT=0
SELFTEST=0
DRY=0
EDITS_ONLY=0
WITH_STEPBACKS=0
APPLY_STEPBACKS=0
DATES=0
REPO=""
JOBS=${CARGO_BUILD_JOBS:-8}   # every cargo call's CARGO_BUILD_JOBS: the caller's, else 8
# The cut instant (DU-23 (a)), an instant with its zone, never a date: the run refuses to start
# before it, and every version a step adds or moves must be published at least 7 days before it.
# The owner's D1 (a) of 2026-10-17 (plan review 2026-10-17 §6): at or after 2026-10-21T19:53Z
# (12:53 PDT), when the whole 2026-10-14 Tauri train is 7 days old; the max date is
# 2026-10-14T19:53:00Z. A later instant is passed with --not-before.
NOT_BEFORE="2026-10-21T19:53Z"
WAIVERS=""
MIN_PASSED=""
ARCHIVE_TO=""
# The Tauri pair (P31a, DU-26 (a'), plan-rev3 rev 3.9): both halves at the 2026-10-14 family, the
# newest set eligible at D1 (a)'s instant (the cut-day compatibility report of 2026-10-17, §2.2 and
# §6.3) — each a parameter (MF-2 item 4). One value per version, so the plugin crate's `=` pin and
# its npm twin are the same string and cannot drift apart (README.md, "The cut instant").
TAURI_PRECISE="2.12.4"     # `cargo update -p tauri --precise`, after the targeted update
DIALOG="2.8.3"             # tauri-plugin-dialog = "=2.8.3" and @tauri-apps/plugin-dialog 2.8.3
JS_API="2.12.4"            # @tauri-apps/api — shares tauri's minor
JS_CLI="2.12.3"            # @tauri-apps/cli
TAURI_CRATE_ARGS=""        # --tauri-crate NAME=VERSION, one a line
JS_PIN_ARGS=""             # --js-pin NAME=VERSION, one a line
DATE_SOURCE=""             # the self-test's stand-in for the registries (lockdates.py --date-source)

usage() {
  cat <<'EOF'
usage: gbase.sh --base <commit> --bun <path> --min-passed <N> [--not-before <UTC>] [--apply-step-backs]
                [--waivers <file>] [--toolchain <ver>] [--steps <N|all>] [--strict] [--dates]
                [--archive-to <dir>] [--repo <path>] [the Tauri and JS pins below]
       gbase.sh --base <commit> ... --dry-run       print the plan after the pre-flight checks; exit 0
       gbase.sh --base <commit> ... --edits-only    export, apply the steps' edits and patches, run no cargo
       gbase.sh --self-test [--toolchain <ver>]     prove the failure modes and assertions on toy workspaces

  --base <commit>      the branch point to export (`git archive`); required unless --self-test
  --not-before <UTC>   the cut instant, written with its zone (`2026-10-21T19:53Z`, or `…:00Z`): a run
                       refuses to start before it (exit 2), and every version a step adds or moves must
                       be published at or before it minus 7 days (DU-23). Default 2026-10-21T19:53Z
                       (12:53 PDT), the owner's D1 (a) — max date 2026-10-14T19:53:00Z. --dry-run and
                       --edits-only check its form only.
  --apply-step-backs   take the freshness gate's step-backs: each `cargo update -p name@locked --precise
                       <target>` the gate names, one at a time, dependents first, the gate run again
                       after each (P04's rule, DU-23); a first entry with nothing eligible to step
                       back to ends the run; without it a young entry ends the run with the
                       sequence in its log
  --waivers <file>     recorded DU-23 (c) waivers, one `name@version  reason` a line: the gate admits
                       exactly those versions (none is recorded today)
  --min-passed <N>     the floor of the suite's passed count (required when the final phase runs):
                       the base's own count, less the pins expected to fail (README.md)
  --toolchain <ver>    the Rust toolchain the wave pins and every cargo call uses (default 1.99.0;
                       `cargo +<ver>` must already be installed — this script installs nothing)
  --bun <path>         the Bun binary for the four GUI gates (required when the final phase runs)
  --steps <N|all>      run the steps up to N (1, 1b, 2..8; 1b is the Tauri pair, after 1) and stop
                       before the final phase (default: all)
  --strict             a lock difference that is not rmcp's 3.5.0 -> 3.5.1 (DU-25 (v4), steps 6-8)
                       or, at step 8, serde-saphyr's and granit-parser's 1.3.0 -> 2.0.0 (D4 (a)) is
                       a failure, not a line for a human
  --with-stepbacks     refused (exit 2): DU-23 (b)'s fixed six-crate list, gone (README.md, "Exit status")
  --dates              after the run, look up on crates.io (User-Agent openalpaca-deps-check) when
                       each version a lock difference introduced was published; off by default
  --archive-to <dir>   also copy the run's evidence-<date>-gbase.tar.gz (logs/ and locks/) into <dir>
  --repo <path>        the repository to export from (default: the one holding this script)

  The pins, D1 (a)'s values by default (each checked against its registry's dates at the pre-flight):
  --tauri <ver>        tauri's `--precise` (2.12.4)          --dialog <ver>   tauri-plugin-dialog's `=`
  --js-api <ver>       @tauri-apps/api (2.12.4)                               and its npm twin (2.8.3)
  --js-cli <ver>       @tauri-apps/cli (2.12.3)
  --tauri-crate NAME=VER   step 1b's exact version for one of tauri-build, tauri-codegen, tauri-macros,
                       tauri-runtime, tauri-runtime-wry, tauri-utils, tauri-plugin, tauri-plugin-fs
                       (default: what locks/d1-lock-01b-pair.lock holds; required for all eight
                       when --tauri is not that lock's tauri)
  --js-pin NAME=VER    the P31/P32/P33 pin of one package of the final phase's JS set

Exit status: 0 — every step and gate passed (lock differences, if any, were printed, and under
--strict there were none); 1 — the first failed command, anchor, patch, comparison or gate (the
message names it and the log), a young entry the gate admits no step-back for, or one without
--apply-step-backs; 2 — a usage error, --with-stepbacks, a missing prerequisite, or a run started
before --not-before.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --base) BASE=${2:?--base needs a commit}; shift 2 ;;
    --toolchain) TOOLCHAIN=${2:?--toolchain needs a version}; shift 2 ;;
    --bun) BUN=${2:?--bun needs a path}; shift 2 ;;
    --steps) STEPS=${2:?--steps needs N or all}; shift 2 ;;
    --strict) STRICT=1; shift ;;
    --self-test) SELFTEST=1; shift ;;
    --dry-run) DRY=1; shift ;;
    --edits-only) EDITS_ONLY=1; shift ;;
    --with-stepbacks) WITH_STEPBACKS=1; shift ;;
    --apply-step-backs) APPLY_STEPBACKS=1; shift ;;
    --not-before) NOT_BEFORE=${2:?--not-before needs a UTC instant}; shift 2 ;;
    --waivers) WAIVERS=${2:?--waivers needs a file}; shift 2 ;;
    --min-passed) MIN_PASSED=${2:?--min-passed needs a count}; shift 2 ;;
    --archive-to) ARCHIVE_TO=${2:?--archive-to needs a directory}; shift 2 ;;
    --tauri) TAURI_PRECISE=${2:?--tauri needs a version}; shift 2 ;;
    --dialog) DIALOG=${2:?--dialog needs a version}; shift 2 ;;
    --js-api) JS_API=${2:?--js-api needs a version}; shift 2 ;;
    --js-cli) JS_CLI=${2:?--js-cli needs a version}; shift 2 ;;
    --tauri-crate) TAURI_CRATE_ARGS="$TAURI_CRATE_ARGS${2:?--tauri-crate needs NAME=VERSION}
"; shift 2 ;;
    --js-pin) JS_PIN_ARGS="$JS_PIN_ARGS${2:?--js-pin needs NAME=VERSION}
"; shift 2 ;;
    --dates) DATES=1; shift ;;
    --repo) REPO=${2:?--repo needs a path}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "gbase: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

case "$STEPS" in
  all|1|1b|2|3|4|5|6|7|8) ;;
  *) echo "gbase: --steps takes 1, 1b, 2..8 or all, not '$STEPS'" >&2; exit 2 ;;
esac

# DU-23 (b)'s fixed step-back list is refused in every mode: it was measured on 2026-10-10 for a cut
# before 2026-10-17, never covered the Tauri pair or the JS set, and is not what is young at any cut.
# The freshness gate (--not-before, --apply-step-backs) measures that on the day instead.
if [ "$WITH_STEPBACKS" -eq 1 ]; then
  cat >&2 <<'EOF'
gbase: --with-stepbacks is refused. It was DU-23 (b)'s fixed list for a cut before 2026-10-17:
after the step-2 refresh, step six crates (toml, toml_edit, uuid, serde_json, tokio-util,
signal-hook) back to releases at least 7 days old. The list was measured on 2026-10-10, never
covered the Tauri pair or the JS set, and is not what is young at any cut. The driver gates every
step against the cut instant instead: --not-before <UTC> names the instant (the owner's D1 (a):
2026-10-21T19:53Z by default), every version a step adds or moves must be published at least
7 days before it, and
--apply-step-backs steps the young ones back one --precise at a time, dependents first, measured
on the day. (README.md, "Exit status" and "The freshness gate".)
EOF
  exit 2
fi

REAL=0   # a run that moves locks: not --self-test, --dry-run or --edits-only
[ "$SELFTEST" -eq 1 ] || [ "$DRY" -eq 1 ] || [ "$EDITS_ONLY" -eq 1 ] || REAL=1

# ---------------------------------------------------------------------------- helpers
T=""              # the canonical scratch root, set by make_root
MODE="apply"      # plan (print) | check (pre-flight: count anchors, check patches) | apply
CURRENT_STEP="setup"
ANCHORS_CHECKED=0
LOCK_UNEXPECTED=0
LOCK_EXPECTED=0
LAST_RC=0
TEST_LINE="not run"
VITEST_LINE="not run"
GUI_LINE="not run"
LOCK_LINE="not compared"
MAX_DATE=""       # NOT_BEFORE minus 7 days: the newest publication time a step may take
STEPBACKS_FILE="" # $T/logs/step-backs.txt: every --precise the freshness gate applied
STEPBACKS_STEP=0
STEPBACKS_TOTAL=0
ARCHIVE=0         # a real run writes the evidence archive at exit
RUN_DATE=""

die() { echo "gbase: FAIL: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || { echo "gbase: missing prerequisite: $1" >&2; exit 2; }; }
usage_fail() { echo "gbase: $*" >&2; exit 2; }

on_err() {
  local rc=$?
  echo "gbase: stopped in '${CURRENT_STEP}' (rc=$rc) at: ${BASH_COMMAND}${T:+  — logs: $T/logs}" >&2
}
trap on_err ERR

say() {
  printf '%s\n' "$*"
  [ -n "$T" ] && [ -d "$T/logs" ] && printf '%s\n' "$*" >> "$T/logs/gbase.summary" || true
}
plan() { printf '%s\n' "$*"; }
first_line() { printf '%s\n' "$1" | head -n 1; }
slug() { printf '%s' "$1" | tr -c 'A-Za-z0-9._-' '-'; }
now() { date -u +%Y-%m-%dT%H:%M:%SZ; }
oneline() { printf '%s\n' "$1" | grep . | tr '\n' ' ' | sed 's/ $//'; }

canon() {
  if command -v realpath >/dev/null 2>&1; then realpath "$1"
  else python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "$1"
  fi
}

# instant_epoch UTC — the seconds of `YYYY-MM-DDTHH:MM[:SS]Z`; anything else (a date alone, no zone,
# an offset, an impossible field) returns 1. python3, not `date`: BSD and GNU `date` parse differently.
instant_epoch() {
  python3 -I -c 'import calendar, re, sys, time
m = re.match(r"^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?Z$", sys.argv[1])
if not m:
    sys.exit(1)
f = [int(g or 0) for g in m.groups()]
t = calendar.timegm((f[0], f[1], f[2], f[3], f[4], f[5], 0, 0, 0))
if list(time.gmtime(t)[:6]) != f:
    sys.exit(1)
print(t)' "$1"
}
iso_of() { python3 -I -c 'import sys, time; print(time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(int(sys.argv[1]))))' "$1"; }

# refuse_before EPOCH — MF-1 item 3: a run does not start before the cut instant (exit 2, nothing made).
refuse_before() {
  local nb=$1 t
  t=$(date -u +%s)
  [ "$t" -ge "$nb" ] && return 0
  echo "gbase: refusing to start before the cut instant $NOT_BEFORE (--not-before); it is $(iso_of "$t") now. The cut is the owner's (DU-23 (a); D1 (a): at or after 2026-10-21T19:53Z); nothing was made." >&2
  exit 2
}

make_root() {
  local d
  d=$(mktemp -d "${TMPDIR:-/tmp}/gbase.XXXXXX")
  T=$(canon "$d")
  mkdir -p "$T/logs" "$T/locks"
  : > "$T/.started"
  STEPBACKS_FILE="$T/logs/step-backs.txt"
  RUN_DATE=$(date -u +%Y-%m-%d)
  say "scratch root: $T  ($(now))"
}

# cargo_cmd TARGET — set CARGO_CMD to the one way every cargo call is made: `nice`, no colour, no
# incremental cache, the caller's jobs, and the build dir pinned to the target dir (MF-5): a
# `[build] build-dir` in the export's .cargo/config.toml (M63) or in the real CARGO_HOME's config
# would otherwise put the intermediates outside $T, where removing the scratch root leaves them.
cargo_cmd() {
  CARGO_CMD=(nice -n 10 env CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0 "CARGO_BUILD_JOBS=$JOBS" \
    "CARGO_TARGET_DIR=$1" "CARGO_BUILD_BUILD_DIR=$1" cargo "+$TOOLCHAIN")
}

# live: the primitives do real work only in apply mode with cargo allowed.
live() { [ "$MODE" = apply ] && [ "$EDITS_ONLY" -eq 0 ]; }

# run_logged_rc NAME CMD... — run CMD, output to $T/logs/NAME.log (its first line the command, its
# last the exit code — tasks/lessons.md, 2026-10), LAST_RC set, never exits.
run_logged_rc() {
  local name=$1; shift
  LAST_RC=0
  if [ "$MODE" = plan ]; then plan "    \$ $*   > logs/$name.log"; return 0; fi
  if [ "$MODE" = check ]; then return 0; fi
  if [ "$EDITS_ONLY" -eq 1 ]; then plan "    (edits-only: not run) $*"; return 0; fi
  local log="$T/logs/$name.log" t0 rc=0
  t0=$(date +%s)
  say "    \$ $*   > logs/$name.log"
  printf '$ %s\n' "$*" > "$log"
  "$@" >> "$log" 2>&1 || rc=$?
  echo "rc=$rc" >> "$log"
  say "      rc=$rc after $(( $(date +%s) - t0 ))s"
  LAST_RC=$rc
}

# run_logged NAME CMD... — as above, and rc != 0 is the end of the run.
run_logged() {
  run_logged_rc "$@"
  if [ "$LAST_RC" -ne 0 ]; then
    local name=$1
    echo "--- last 30 lines of $T/logs/$name.log" >&2
    tail -n 30 "$T/logs/$name.log" >&2 || true
    die "command failed (rc=$LAST_RC): $name"
  fi
}

count_diag() { # count_diag LOG -> "errors=N warnings=M"
  local e w
  e=$(grep -cE '^error(\[|:)' "$1" || true)
  w=$(grep -cE '^warning(\[|:)' "$1" || true)
  printf 'errors=%s warnings=%s' "$e" "$w"
}

# edit FILE OLD_LINE NEW_TEXT — replace the one line equal to OLD_LINE (exactly once) by NEW_TEXT.
edit() {
  local f=$1 old=$2 new=$3 n
  case "$MODE" in
    plan)
      plan "    edit $f: $(first_line "$old")  ->  $(first_line "$new")"
      return 0 ;;
    check)
      n=$(grep -cFx -- "$old" "$f" 2>/dev/null || true); n=${n:-0}
      [ "$n" = 1 ] || die "pre-flight: anchor matched $n times (want 1) in $f: $old"
      ANCHORS_CHECKED=$((ANCHORS_CHECKED + 1))
      return 0 ;;
  esac
  n=$(grep -cFx -- "$old" "$f" 2>/dev/null || true); n=${n:-0}
  [ "$n" = 1 ] || die "anchor matched $n times (want 1) in $f: $old"
  OLD="$old" NEW="$new" perl -pi -e 's/^\Q$ENV{OLD}\E$/$ENV{NEW}/' "$f"
  n=$(grep -cFx -- "$old" "$f" 2>/dev/null || true)
  [ "$n" = 0 ] || die "edit left the old line in $f: $old"
  say "    edited $f: $(first_line "$old")  ->  $(first_line "$new")"
}

# apply_patch NAME — git apply patches/NAME.patch in the export (all-or-nothing).
apply_patch() {
  local p="$HERE/patches/$1.patch"
  [ -f "$p" ] || die "missing patch: $p"
  case "$MODE" in
    plan)
      plan "    patch $1.patch:"
      sed -n 's|^diff --git a/\(.*\) b/.*$|      \1|p' "$p"
      return 0 ;;
    check)
      git apply --check "$p" > "$T/logs/preflight-apply-check-$1.log" 2>&1 \
        || { cat "$T/logs/preflight-apply-check-$1.log" >&2; die "pre-flight: $1.patch does not apply to the base"; }
      return 0 ;;
  esac
  git apply "$p" > "$T/logs/$CURRENT_STEP-apply-$1.log" 2>&1 \
    || { cat "$T/logs/$CURRENT_STEP-apply-$1.log" >&2; die "$1.patch did not apply"; }
  say "    applied $1.patch ($(grep -c '^diff --git' "$p") files)"
}

lock_versions() { # lock_versions NAME [LOCK] -> the versions the lock holds for NAME, one per line
  awk -v p="$1" '$0 == "name = \"" p "\"" { getline; sub(/^version = "/, ""); sub(/"$/, ""); print }' "${2:-Cargo.lock}"
}

ere_exact() { printf '^%s$' "$(printf '%s' "$1" | sed 's/[.]/[.]/g')"; }   # 2.8.2 -> ^2[.]8[.]2$
ere_minor() { printf '^%s[.]' "$(printf '%s' "$1" | sed 's/[.]/[.]/g')"; } # 2.12  -> ^2[.]12[.]

# expect_lock NAME ERE WHY — the lock holds NAME at exactly one version and it matches ERE;
# anything else ends the run.
expect_lock() {
  local name=$1 ere=$2 why=$3 vers n
  vers=$(lock_versions "$name")
  n=$(printf '%s\n' "$vers" | grep -c . || true)
  if [ "$n" = 1 ] && printf '%s\n' "$vers" | grep -Eq -- "$ere"; then return 0; fi
  die "$why: the lock holds $name at '$(printf '%s ' $vers)' (want exactly one version matching $ere)"
}

# pair_check WHERE API DIALOG — DU-26's invariant, P04's R20 check: the lock holds one `tauri`,
# sharing its minor with @tauri-apps/api API, and one `tauri-plugin-dialog`, equal to
# @tauri-apps/plugin-dialog DIALOG. A broken pair ends the run: Rust never moves a Tauri minor
# alone. `tauri = "~2.12"` and `tauri-build = "~2.7"` are tilde requirements (the minor held, plan-rev3
# rev 3.10 — a caret would admit 2.13) and the plugin's `=` holds it exactly; this check is the backstop.
pair_check() {
  local where=$1 api=$2 dialog=$3 t d
  [ "$MODE" = check ] && return 0
  if ! live; then plan "    pair check ($where): tauri shares a minor with @tauri-apps/api $api; tauri-plugin-dialog = @tauri-apps/plugin-dialog $dialog"; return 0; fi
  expect_lock tauri "$(ere_minor "${api%.*}")" "$where: the Tauri pair is broken (@tauri-apps/api $api)"
  expect_lock tauri-plugin-dialog "$(ere_exact "$dialog")" "$where: the Tauri pair is broken (@tauri-apps/plugin-dialog $dialog)"
  t=$(lock_versions tauri); d=$(lock_versions tauri-plugin-dialog)
  say "    the Tauri pair holds ($where): tauri $t ~ @tauri-apps/api $api; tauri-plugin-dialog $d = @tauri-apps/plugin-dialog $dialog"
}

tauri_subgraph() { # one line: the Tauri sub-graph's versions in the lock (P04's and P31a's bodies record it)
  local p set=""
  for p in tauri tauri-build tauri-plugin-dialog tauri-plugin-fs tauri-utils tauri-runtime tauri-runtime-wry \
           tauri-codegen tauri-macros tauri-plugin wry tao swift-rs; do
    set="$set$p $(lock_versions "$p" | tr '\n' '/' | sed 's|/$||')  "
  done
  printf '%s\n' "$set"
}

# ---------------------------------------------------------------------------- the Tauri set
# MF-1 item 2: step 1b holds every crate of the pair's sub-graph at exactly one expected version,
# each published at or before the max date. tauri and tauri-plugin-dialog are --tauri / --dialog;
# the eight internals default to what step 1b's reference lock holds (derived for D1 (a) on
# 2026-10-17: the 2026-10-14 family over HEAD's lock, resolved under 1.99 — README.md, "The pair
# step's reference"), so a re-derived reference re-anchors them in one file; --tauri-crate
# overrides one.
TAURI_INTERNALS="tauri-build tauri-codegen tauri-macros tauri-runtime tauri-runtime-wry tauri-utils tauri-plugin tauri-plugin-fs"
TAURI_REF="$HERE/locks/d1-lock-01b-pair.lock"
TAURI_SET=""

tauri_expected() { printf '%s\n' "$TAURI_SET" | awk -v n="$1" '$1 == n { print $2 }'; }

build_tauri_set() {
  local line name v ref_tauri missing="" overrides=""
  [ -f "$TAURI_REF" ] || die "missing shipped file $TAURI_REF"
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    name=${line%%=*}; v=${line#*=}
    case " $TAURI_INTERNALS " in *" $name "*) ;; *) usage_fail "--tauri-crate takes one of: $TAURI_INTERNALS (tauri and tauri-plugin-dialog are --tauri and --dialog), not '$line'" ;; esac
    printf '%s' "$v" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || usage_fail "--tauri-crate $line: not a version"
    overrides="$overrides$name $v
"
  done <<EOF
$TAURI_CRATE_ARGS
EOF
  ref_tauri=$(lock_versions tauri "$TAURI_REF")
  TAURI_SET="tauri $TAURI_PRECISE"
  for name in $TAURI_INTERNALS; do
    v=$(printf '%s' "$overrides" | awk -v n="$name" '$1 == n { v = $2 } END { print v }')
    if [ -z "$v" ]; then
      [ "$ref_tauri" = "$TAURI_PRECISE" ] || missing="$missing $name"
      v=$(lock_versions "$name" "$TAURI_REF")
      [ "$(printf '%s\n' "$v" | grep -c .)" = 1 ] || die "the reference lock $TAURI_REF holds $name at '$(oneline "$v")', not at one version"
    fi
    TAURI_SET="$TAURI_SET
$name $v"
  done
  TAURI_SET="$TAURI_SET
tauri-plugin-dialog $DIALOG"
  [ -z "$missing" ] || usage_fail "--tauri $TAURI_PRECISE is not the reference lock's tauri ($ref_tauri), whose internals belong to that tauri: give --tauri-crate NAME=VERSION for$missing, or re-derive locks/d1-lock-01b-pair.lock (README.md, \"The pair step's reference\")"
}

# ---------------------------------------------------------------------------- the JS set
# P31a (the three @tauri-apps/* pins, the pair's JS half — its Rust half is step 1b) + P31
# (minor/patch pins, measured set under DU-23 (a)) + P32 (Vite 8, Vitest 5) + P33
# (@vitejs/plugin-react) — stage B2's packages [M-oct10: gui-B-pins.log] at the newest release
# eligible at D1 (a)'s instant (the cut-day compatibility report of 2026-10-17, §2.4 and §6.2).
# One row a pin: the base's name and version (the anchor), the new name and its default version
# (a --js-pin overrides it), and `,` when the package.json line ends in a comma, `-` when it does not.
JS_PINS='@tanstack/react-query 5.102.8 @tanstack/react-query 5.104.2 ,
dompurify 3.4.14 dompurify 3.4.17 ,
marked 18.0.11 marked 18.1.1 ,
react 19.2.8 react 19.3.1 ,
react-dom 19.2.8 react-dom 19.3.1 ,
tailwind-merge 3.6.0 tailwind-merge 3.7.0 ,
@testing-library/dom 10.4.1 @testing-library/dom 10.4.2 ,
@testing-library/user-event 14.6.6 @testing-library/user-event 14.6.8 ,
@types/node 26.4.0 @types/node 26.6.5 ,
@types/react 19.2.18 @types/react 19.3.0 ,
@types/react-dom 19.2.5 @types/react-dom 19.3.0 ,
@vitejs/plugin-react-swc 4.3.3 @vitejs/plugin-react 6.1.2 ,
jsdom 30.0.1 jsdom 30.1.2 ,
prettier 3.9.6 prettier 3.9.10 ,
vite 7.3.6 vite 8.3.4 ,
vitest 4.1.11 vitest 5.0.4 -'
JS_OVERRIDES=""

check_js_pins() {
  local line name v
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    name=${line%=*}; v=${line##*=}
    printf '%s\n' "$JS_PINS" | awk -v n="$name" '$3 == n { f = 1 } END { exit !f }' \
      || usage_fail "--js-pin takes one of the final phase's P31/P32/P33 packages ($(printf '%s\n' "$JS_PINS" | awk '{ printf "%s%s", s, $3; s = " " }'); the @tauri-apps pins are --js-api, --js-cli and --dialog), not '$line'"
    printf '%s' "$v" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' || usage_fail "--js-pin $line: not a version"
    JS_OVERRIDES="$JS_OVERRIDES$name $v
"
  done <<EOF
$JS_PIN_ARGS
EOF
}
js_pin_version() { # js_pin_version NAME DEFAULT
  local v
  v=$(printf '%s' "$JS_OVERRIDES" | awk -v n="$1" '$1 == n { v = $2 } END { print v }')
  printf '%s\n' "${v:-$2}"
}

# check_args — every usage error, before a scratch root exists; then, for a real run, the cut.
check_args() {
  local v
  for v in "$TAURI_PRECISE" "$DIALOG" "$JS_API" "$JS_CLI"; do
    printf '%s' "$v" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || usage_fail "--tauri, --dialog, --js-api and --js-cli take a version, not '$v'"
  done
  [ -z "$MIN_PASSED" ] || printf '%s' "$MIN_PASSED" | grep -Eq '^[0-9]+$' || usage_fail "--min-passed takes a count, not '$MIN_PASSED'"
  if [ -n "$WAIVERS" ]; then
    [ -f "$WAIVERS" ] && [ -r "$WAIVERS" ] || usage_fail "--waivers $WAIVERS is not a readable file"
    WAIVERS=$(canon "$WAIVERS")
  fi
  if [ -n "$ARCHIVE_TO" ]; then
    [ -d "$ARCHIVE_TO" ] || usage_fail "--archive-to $ARCHIVE_TO is not a directory"
    ARCHIVE_TO=$(canon "$ARCHIVE_TO")
  fi
  check_js_pins
  build_tauri_set
  TAURI_MINOR=${TAURI_PRECISE%.*}
  TAURI_BUILD_MINOR=$(tauri_expected tauri-build); TAURI_BUILD_MINOR=${TAURI_BUILD_MINOR%.*}
  TAURI_REQ="~$TAURI_MINOR"              # [workspace.dependencies] tauri = "~2.12" (tilde: the minor held)
  TAURI_BUILD_REQ="~$TAURI_BUILD_MINOR"  # [workspace.dependencies] tauri-build = "~2.7"
  NOT_BEFORE_EPOCH=$(instant_epoch "$NOT_BEFORE") \
    || usage_fail "--not-before takes the cut instant with its zone, YYYY-MM-DDTHH:MM[:SS]Z (UTC), not '$NOT_BEFORE': a date alone admits releases early in one zone or the other (plan review 2026-10-17, MF-1)"
  MAX_DATE=$(iso_of $(( NOT_BEFORE_EPOCH - 7 * 86400 )))
  [ "$REAL" -eq 0 ] || refuse_before "$NOT_BEFORE_EPOCH"
  return 0
}

# compare_lock LABEL EXPECTED ACTUAL [--expected LABEL=SPECS]... — print the differences;
# an unexpected one fails the run under --strict. A SPEC is a name (any difference on it) or
# name:OLD->NEW (exactly that move) — lockdiff.py's docstring.
compare_lock() {
  local label=$1 exp=$2 act=$3; shift 3
  local out rc=0 head moved added removed unexpected total
  out="$T/logs/lockdiff-$(slug "$label").txt"
  if cmp -s "$exp" "$act"; then
    say "    lock: byte-identical to $(basename "$exp")"
    { echo "== $label: $(basename "$act") equals $(basename "$exp")"; echo; } >> "$T/logs/lock-differences.txt"
    return 0
  fi
  python3 -I "$HERE/lockdiff.py" "$@" "$exp" "$act" > "$out" 2>&1 || rc=$?
  # lockdiff.py exits 1 for "a difference was found" and 2 for anything else (a usage error, a
  # file that is not a Cargo.lock, an unforeseen error); and whatever its status, output whose
  # first line is not the summary is no comparison — a lockdiff.py that dies before main (a
  # syntax error) exits 1 with Python's error report, which the field-wise parse below would
  # read as 0 0 0 0, "the same (name, version) set".
  [ "$rc" -le 1 ] || { cat "$out" >&2; die "lockdiff.py failed (rc=$rc): no comparison of $(basename "$exp") and $(basename "$act")"; }
  head=$(head -n 1 "$out")
  case "$head" in
    'moved: '*) ;;
    *) cat "$out" >&2; die "lockdiff.py gave no summary (rc=$rc): no comparison of $(basename "$exp") and $(basename "$act")" ;;
  esac
  # field-wise, never by regex: a `.*moved: ` pattern also matches inside `removed: `
  set -- $(printf '%s\n' "$head" | awk '{ for (i = 1; i < NF; i++) { if ($i == "moved:") m = $(i+1); if ($i == "added:") a = $(i+1); if ($i == "removed:") r = $(i+1); if ($i == "unexpected:") u = $(i+1) } } END { print m + 0, a + 0, r + 0, u + 0 }')
  moved=$1; added=$2; removed=$3; unexpected=$4
  total=$(( moved + added + removed ))
  { echo "== $label: $(basename "$exp") vs $(basename "$act")"; cat "$out"; echo; } >> "$T/logs/lock-differences.txt"
  if [ "$total" -eq 0 ]; then
    say "    lock: same (name, version) set as $(basename "$exp") (bytes differ)"
    return 0
  fi
  say "    lock: differs from $(basename "$exp") — $head"
  sed 's/^/      /' "$out" | tail -n +2
  LOCK_UNEXPECTED=$(( LOCK_UNEXPECTED + ${unexpected:-0} ))
  LOCK_EXPECTED=$(( LOCK_EXPECTED + total - ${unexpected:-0} ))
  if [ "${unexpected:-0}" -gt 0 ] && [ "$STRICT" -eq 1 ]; then
    die "$label: $unexpected unexpected lock difference(s) under --strict (see $out)"
  fi
  return 0
}

# snapshot_compare N SHIPPED [--expected LABEL=SPECS]... — copy the lock to locks/step-N.lock and compare.
snapshot_compare() {
  local n=$1 shipped=$2; shift 2
  [ "$MODE" = check ] && return 0
  if ! live; then
    plan "    lock: snapshot locks/step-$n.lock, compare with $shipped${*:+  (expected differences: $*)}"
    return 0
  fi
  cp Cargo.lock "$T/locks/step-$n.lock"
  compare_lock "step-$n" "$HERE/locks/$shipped" "$T/locks/step-$n.lock" "$@"
}

begin_step() {
  CURRENT_STEP="step-$1"
  if [ "$MODE" = check ]; then return 0; fi
  say ""
  say "== step $1: $2   ($(now))"
}

prev_lock() { # prev_lock STEP -> the snapshot of the lock the step starts from
  local p=0 n
  for n in $STEP_ORDER; do
    [ "$n" = "$1" ] && break
    p=$n
  done
  if [ "$p" = 0 ]; then printf '%s\n' "$T/locks/step-0-base.lock"; else printf '%s\n' "$T/locks/step-$p.lock"; fi
}

# ---------------------------------------------------------------------------- the freshness gate
# DU-23, enforced (plan review 2026-10-17, MF-2): every (name, version) a step adds or moves against
# the lock it started from must be published on crates.io at or before MAX_DATE (the cut instant
# minus 7 days), or be a recorded DU-23 (c) waiver. lockdates.py --gate decides and names, for each
# young entry, the newest eligible version to step back to, dependents first. Without
# --apply-step-backs a young entry ends the run there; with it the driver runs exactly the first
# --precise the gate names, gates again (the steps cascade), and repeats — never a target of its own,
# and never past a first entry with nothing eligible to step back to, which ends the run.
regflags_init() {
  LOCKDATES=(python3 -I "$HERE/lockdates.py")
  REGFLAGS=(--max-date "$MAX_DATE")
  if [ -n "$WAIVERS" ]; then REGFLAGS+=(--waivers "$WAIVERS"); fi
  if [ -n "$DATE_SOURCE" ]; then REGFLAGS+=(--date-source "$DATE_SOURCE"); else REGFLAGS+=(--cache "$T/logs/registry-cache.json"); fi
}

# freshness STEP PARENT [CHECK] — the gate on the step's lock against PARENT; CHECK names the step's
# `cargo check` log, run again (--locked) after any step-back, for a step whose check resolved the lock.
freshness() {
  local n=$1 parent=$2 recheck=${3:-} round=0 limit=0 log line spec target young applied="" published
  STEPBACKS_STEP=0
  [ "$MODE" = check ] && return 0
  if ! live; then
    if [ "$APPLY_STEPBACKS" -eq 1 ]; then
      plan "    freshness gate: what step $n adds or moves against $(basename "$parent") published at or before $MAX_DATE, or waived; a younger entry is stepped back (the gate's --precise, one at a time, dependents first)${recheck:+, then \`check --locked\` again}"
    else
      plan "    freshness gate: what step $n adds or moves against $(basename "$parent") published at or before $MAX_DATE, or waived; a younger entry ends the run with its --precise sequence (step-backs only under --apply-step-backs)"
    fi
    return 0
  fi
  while :; do
    log="fresh-$(slug "$n")-$round"
    run_logged_rc "$log" "${LOCKDATES[@]}" --gate "${REGFLAGS[@]}" "$parent" Cargo.lock
    if [ "$LAST_RC" -gt 1 ]; then
      tail -n 20 "$T/logs/$log.log" >&2 || true
      die "step $n: the freshness gate gave no verdict (rc=$LAST_RC; a failed registry lookup is no verdict): logs/$log.log"
    fi
    young=$(grep -c '^  TOO YOUNG ' "$T/logs/$log.log" || true)
    [ "$LAST_RC" -eq 0 ] && break
    [ "$young" -gt 0 ] || die "step $n: the freshness gate refused with no young entry listed (logs/$log.log)"
    if [ "$APPLY_STEPBACKS" -eq 0 ]; then
      sed -n '/^the --precise sequence/,$p' "$T/logs/$log.log" | grep -Ev '^(no-)?step-back: |^rc=' >&2 || true
      die "step $n: $young version(s) younger than 7 days at the cut (published after $MAX_DATE): the --precise sequence above, dependents first, is in logs/$log.log — rerun with --apply-step-backs to take it (P04's rule, DU-23), or list a recorded DU-23 (c) waiver in --waivers"
    fi
    [ "$round" -gt 0 ] || limit=$(( young * 2 + 10 ))
    [ "$round" -lt "$limit" ] || die "step $n: $young young version(s) left after $round step-backs: they do not converge (logs/$log.log)"
    # The first entry of the sequence, of either kind — never the first step-back past a no-step-back,
    # which could step a dependency back before its dependent. A no-step-back there has no young
    # dependent above it whose step-back could drop it, so nothing can cascade to it: the run ends.
    # One further down is left to the cascade (a dependent's older release may not need it at all).
    line=$(grep -m 1 -E '^(no-)?step-back: ' "$T/logs/$log.log" || true)
    case $line in
      step-back:\ *) line=${line#step-back: } ;;
      no-step-back:\ *)
        die "step $n: ${line#no-step-back: }, the first entry of the --precise sequence (dependents first), is younger than 7 days with no eligible version to step back to (every such entry: $(sed -n 's/^no-step-back: //p' "$T/logs/$log.log" | tr '\n' ' ' | sed 's/ $//')) — a recorded DU-23 (c) waiver, or a later cut (logs/$log.log)" ;;
      *) die "step $n: the freshness gate listed $young young version(s) and no step-back line (logs/$log.log)" ;;
    esac
    spec=${line% *}; target=${line##* }
    case " $applied " in *" $spec->$target "*) die "step $n: the gate named $spec -> $target again after it was applied: the step-backs do not converge (logs/$log.log)" ;; esac
    applied="$applied $spec->$target"
    published=$(awk -v n="${spec%@*}" -v v="${spec##*@}" '$1 == "TOO" && $3 == n && $4 == v { print $5 }' "$T/logs/$log.log")
    run_logged_rc "$log-precise" "${CARGO[@]}" update -p "$spec" --precise "$target"
    if [ "$LAST_RC" -ne 0 ]; then
      tail -n 20 "$T/logs/$log-precise.log" >&2 || true
      die "step $n: cargo refused the step-back $spec -> $target (rc=$LAST_RC) — a named blocker: a recorded DU-23 (c) waiver, or a later cut (logs/$log-precise.log)"
    fi
    printf 'step %s: cargo update -p %s --precise %s   (%s published %s, after %s; logs/%s.log)\n' \
      "$n" "$spec" "$target" "$spec" "${published:-?}" "$MAX_DATE" "$log" >> "$STEPBACKS_FILE"
    say "    stepped back: $spec -> $target  (published ${published:-?}, after $MAX_DATE)"
    STEPBACKS_STEP=$((STEPBACKS_STEP + 1)); STEPBACKS_TOTAL=$((STEPBACKS_TOTAL + 1))
    round=$((round + 1))
  done
  say "    freshness ($MAX_DATE): $(grep -m 1 '^  added or moved:' "$T/logs/$log.log" | sed 's/^ *//')$( [ "$STEPBACKS_STEP" -eq 0 ] || printf '; %s step-back(s) applied (%s)' "$STEPBACKS_STEP" "$STEPBACKS_FILE")"
  if [ -n "$recheck" ] && [ "$STEPBACKS_STEP" -gt 0 ]; then
    run_logged "$recheck-after-step-backs" "${CARGO[@]}" check --locked --workspace --all-targets --exclude openalpaca_gui
    say "    $(count_diag "$T/logs/$recheck-after-step-backs.log")"
  fi
}

# expect_tauri_set WHERE — MF-1 item 2: every crate of the pair's sub-graph held once, at exactly
# its expected version, and each published at or before the max date (or a recorded waiver).
expect_tauri_set() {
  local where=$1 name v pins=""
  [ "$MODE" = check ] && return 0
  if ! live; then
    plan "    the Tauri set ($where): exactly $(printf '%s\n' "$TAURI_SET" | awk '{ printf "%s%s %s", s, $1, $2; s = ", " }'); each published at or before $MAX_DATE"
    return 0
  fi
  for name in tauri $TAURI_INTERNALS tauri-plugin-dialog; do
    v=$(tauri_expected "$name")
    expect_lock "$name" "$(ere_exact "$v")" "$where: the Tauri set (MF-1)"
    pins="$pins $name@$v"
  done
  run_logged_rc "$(slug "$where")-tauri-dates" "${LOCKDATES[@]}" --pins crates "${REGFLAGS[@]}" $pins
  case "$LAST_RC" in
    0) ;;
    1) grep '^  TOO YOUNG ' "$T/logs/$(slug "$where")-tauri-dates.log" >&2 || true
       die "$where: a crate of the Tauri set is younger than 7 days at the cut (published after $MAX_DATE; logs/$(slug "$where")-tauri-dates.log)" ;;
    *) die "$where: no verdict on the Tauri set's dates (rc=$LAST_RC; logs/$(slug "$where")-tauri-dates.log)" ;;
  esac
  say "    the Tauri set holds ($where): $(oneline "$(printf '%s\n' "$TAURI_SET" | awk '{ print $1 " " $2 "," }')") each published at or before $MAX_DATE"
}

# ---------------------------------------------------------------------------- the steps
# Every manifest version below is the measured re-cut's (plan-rev3 §5.3, DU-23 (a)) but step 8's
# serde-saphyr, D4 (a)'s 2.0 (the owner's ruling of 2026-10-19; the re-cut took 1.3); the shipped
# locks under locks/ describe this sequence at 7295f91b — the re-cut's un-held snapshots — and for
# step 1b the 2026-10-14 family over HEAD's lock, derived for D1 (a) on 2026-10-17 (README.md,
# "The pair step's reference").

# DU-25 (v4), rev 3.9: rmcp is taken at the newest 3.5.x — no --precise — so from step 6 on the
# lock differs from the measured snapshots (cut with `--precise 3.5.0`) by exactly this move.
RMCP_EXPECT="DU-25(v4)=rmcp:3.5.0->3.5.1,rmcp-macros:3.5.0->3.5.1"

# D4 (a), the owner's ruling of 2026-10-19: step 8 takes serde-saphyr 2.0 (P30) where the re-cut took
# 1.3, so step 8's lock differs from the final snapshot by exactly these two moves as well (the
# cut-day compatibility report of 2026-10-17, §5: 2.0.0 moves serde-saphyr and granit-parser only).
SAPHYR_EXPECT="D4(a)=serde-saphyr:1.3.0->2.0.0,granit-parser:1.3.0->2.0.0"

step_1() {
  begin_step 1 "toolchain pin and rust-version (P03)"
  edit rust-toolchain.toml 'channel = "1.93.0"' "channel = \"$TOOLCHAIN\""
  edit Cargo.toml 'rust-version = "1.93"' "rust-version = \"${TOOLCHAIN%.*}\""
  run_logged 01-check "${CARGO[@]}" check --locked --workspace --all-targets --exclude openalpaca_gui
  if live; then
    say "    $(count_diag "$T/logs/01-check.log")"
    cmp -s Cargo.lock "$T/locks/step-0-base.lock" || die "step 1 changed the lock although it ran --locked"
  fi
  snapshot_compare 1 recut-lock-01-toolchain.lock
}

step_1b() {
  begin_step 1b "the Tauri pair's Rust half: tauri $TAURI_REQ, tauri-build $TAURI_BUILD_REQ, tauri-plugin-dialog =$DIALOG in [workspace.dependencies]  (P31a)"
  edit Cargo.toml 'tauri-plugin-dialog = "2"' "tauri = \"$TAURI_REQ\"
tauri-build = \"$TAURI_BUILD_REQ\"
tauri-plugin-dialog = \"=$DIALOG\""
  edit apps/openalpaca-gui/src-tauri/Cargo.toml 'tauri-build = { version = "2", features = [] }' 'tauri-build.workspace = true'
  edit apps/openalpaca-gui/src-tauri/Cargo.toml 'tauri = { version = "2", features = [] }' 'tauri.workspace = true'
  run_logged 01b-pair-update "${CARGO[@]}" update -p tauri -p tauri-build -p tauri-plugin-dialog
  live && say "    tauri after the targeted update: $(lock_versions tauri | tr '\n' ' ')(the newest $TAURI_MINOR.x the registry carries, eligible at the cut or not; --precise sets the one the run takes)"
  run_logged 01b-pair-precise "${CARGO[@]}" update -p tauri --precise "$TAURI_PRECISE"
  freshness 1b "$(prev_lock 1b)"
  live && say "    Tauri sub-graph after the pair (P31a's body records it): $(tauri_subgraph)"
  expect_tauri_set "step 1b"
  pair_check "step 1b" "$JS_API" "$DIALOG"
  run_logged 01b-check "${CARGO[@]}" check --workspace --all-targets --exclude openalpaca_gui
  live && say "    $(count_diag "$T/logs/01b-check.log")"
  snapshot_compare 1b d1-lock-01b-pair.lock
}

step_2() {
  begin_step 2 "semver refresh — a plain cargo update, then the freshness gate  (P04)"
  run_logged 02-update "${CARGO[@]}" update
  # `tauri = "~2.12"` and `tauri-build = "~2.7"` are tilde requirements: the refresh may take a newer
  # patch, never a new minor; the gate admits only an eligible one, and the set must stay step 1b's.
  freshness 2 "$(prev_lock 2)"
  live && say "    Tauri sub-graph after the refresh (P04's body records it): $(tauri_subgraph)"
  expect_tauri_set "step 2"
  pair_check "step 2" "$JS_API" "$DIALOG"
  run_logged 02-check "${CARGO[@]}" check --workspace --all-targets --exclude openalpaca_gui
  live && say "    $(count_diag "$T/logs/02-check.log")"
  snapshot_compare 2 recut-lock-02-update.lock
}

step_3() {
  begin_step 3 "storage: rusqlite 0.40.2, lru 0.18, sysinfo 0.39.6  (P06-P08)"
  edit Cargo.toml 'rusqlite = { version = "0.38.0", features = ["bundled"] }' 'rusqlite = { version = "0.40.2", features = ["bundled"] }'
  edit Cargo.toml 'lru = "0.12"' 'lru = "0.18"'
  edit Cargo.toml 'sysinfo = "0.38.1"' 'sysinfo = "0.39.6"'
  run_logged 03-check "${CARGO[@]}" check --workspace --all-targets --exclude openalpaca_gui
  live && say "    $(count_diag "$T/logs/03-check.log")"
  freshness 3 "$(prev_lock 3)" 03-check
  snapshot_compare 3 recut-lock-03-storage.lock
}

step_4() {
  begin_step 4 "secrets and hashing: base64 0.23.1, sha2 0.11 (eleven digest sites), aes-gcm 0.11, keyring 4.2  (P09-P12)"
  edit Cargo.toml 'base64 = "0.22.1"' 'base64 = "0.23.1"'
  edit Cargo.toml 'sha2 = "0.10"' 'sha2 = "0.11"'
  edit Cargo.toml 'aes-gcm = "0.10"' 'aes-gcm = "0.11"'
  edit Cargo.toml 'keyring = { version = "3", features = ["apple-native", "windows-native", "sync-secret-service"] }' \
'# v4 moved the platform stores into separate crates; the `v1` feature is the
# old `Entry` API over the native store per OS (macOS login keychain, Windows
# Credential Manager, Secret Service over zbus on other unix).
keyring = { version = "4.2", default-features = false, features = ["v1"] }'
  apply_patch step-4-secrets
  run_logged 04-check "${CARGO[@]}" check --workspace --all-targets --exclude openalpaca_gui
  live && say "    $(count_diag "$T/logs/04-check.log")"
  freshness 4 "$(prev_lock 4)" 04-check
  snapshot_compare 4 recut-lock-04-secrets.lock
}

step_5() {
  begin_step 5 "documents and sniffing: infer 0.22, pdf-extract 0.12, zip 8.6 (deflate only), calamine 0.36  (P15, P16, P18)"
  edit Cargo.toml 'infer = "0.16"' 'infer = "0.22"'
  edit Cargo.toml 'pdf-extract = "0.8"' 'pdf-extract = "0.12"'
  edit Cargo.toml 'zip = "2"' 'zip = { version = "8.6", default-features = false, features = ["deflate"] }'
  edit Cargo.toml 'calamine = "0.26"' 'calamine = "0.36"'
  run_logged 05-check "${CARGO[@]}" check --workspace --all-targets --exclude openalpaca_gui
  live && say "    $(count_diag "$T/logs/05-check.log")"
  freshness 5 "$(prev_lock 5)" 05-check
  snapshot_compare 5 recut-lock-05-documents.lock
}

step_6() {
  begin_step 6 "MCP client: rmcp 0.16 -> 3.5.x (the newest, no --precise), the source hunks  (P19)"
  # The manifest line is the measured diff's (`3.5.0`, the floor); P19's own commit writes the
  # floor equal to the version taken (R05). The changed requirement is resolved by the re-cut's own
  # first command less its --precise (recut-06-rmcp-update.log: rmcp and rmcp-macros 0.16.0 -> 3.5.1,
  # process-wrap 10.0.1, darling 0.23 gone); rev 3.1-3.8 then stepped both back to 3.5.0.
  edit crates/openalpaca_mcp/Cargo.toml \
    'rmcp = { version = "0.16", features = ["client", "transport-child-process", "transport-streamable-http-client-reqwest"] }' \
    'rmcp = { version = "3.5.0", features = ["client", "transport-child-process", "transport-streamable-http-client-reqwest"] }'
  apply_patch step-6-rmcp
  run_logged 06-rmcp-update "${CARGO[@]}" update -p rmcp
  freshness 6 "$(prev_lock 6)"
  if live; then
    local r m
    r=$(lock_versions rmcp); m=$(lock_versions rmcp-macros)
    say "    rmcp: $(printf '%s ' $r)rmcp-macros: $(printf '%s ' $m)(the measured cut's first resolution: 3.5.1 / 3.5.1)"
    expect_lock rmcp '^3[.]5[.]' "step 6"
    expect_lock rmcp-macros '^3[.]5[.]' "step 6"
    [ "$r" = "$m" ] || die "step 6: rmcp $r and rmcp-macros $m differ"
  fi
  run_logged 06-check "${CARGO[@]}" check --workspace --all-targets --exclude openalpaca_gui
  live && say "    $(count_diag "$T/logs/06-check.log")"
  snapshot_compare 6 recut-lock-06-rmcp.lock --expected "$RMCP_EXPECT"
}

step_7() {
  begin_step 7 "web and CLI: tower-http 0.7.1, tokio-tungstenite 0.30.0, rustyline 18  (P25-P27)"
  edit Cargo.toml 'tower-http = { version = "0.6.8", features = ["cors"] }' 'tower-http = { version = "0.7.1", features = ["cors"] }'
  edit Cargo.toml 'tokio-tungstenite = "0.28.0"' 'tokio-tungstenite = "0.30.0"'
  edit Cargo.toml 'rustyline = "17"' 'rustyline = "18"'
  run_logged 07-check "${CARGO[@]}" check --workspace --all-targets --exclude openalpaca_gui
  live && say "    $(count_diag "$T/logs/07-check.log")"
  freshness 7 "$(prev_lock 7)" 07-check
  snapshot_compare 7 recut-lock-07-web.lock --expected "$RMCP_EXPECT"
}

step_8() {
  begin_step 8 "configuration formats: toml 1.1, serde_yaml -> serde-saphyr 2.0  (P29, P30)"
  edit Cargo.toml 'toml = "0.9"' 'toml = "1.1"'
  edit Cargo.toml 'serde_yaml = "0.9"' $'# SKILL.md frontmatter. Replaces the archived serde_yaml 0.9: maintained, pure\n# Rust, and set as close to serde_yaml\'s reading as its options go\n# (`yaml_options` in openalpaca_core::middleware::skill).\nserde-saphyr = "2.0"'
  edit crates/openalpaca_core/Cargo.toml 'serde_yaml.workspace = true' 'serde-saphyr.workspace = true'
  apply_patch step-8-config
  run_logged 08-check "${CARGO[@]}" check --workspace --all-targets --exclude openalpaca_gui
  live && say "    $(count_diag "$T/logs/08-check.log")"
  freshness 8 "$(prev_lock 8)" 08-check
  # The step-8 snapshot IS the wave's final lock: compared with the re-cut's final snapshot
  # (recut-lock-99-final.lock, byte-identical to its recut-lock-08-config.lock), which holds
  # serde-saphyr and granit-parser 1.3.0 — cut on 2026-10-10, before D4 (a) — hence SAPHYR_EXPECT.
  snapshot_compare 8 recut-lock-99-final.lock --expected "$RMCP_EXPECT" --expected "$SAPHYR_EXPECT"
}

STEP_ORDER="1 1b 2 3 4 5 6 7 8"   # 1b, the Tauri pair (P31a), lands right after the toolchain (P03)

run_steps() { # run_steps LAST — the steps of STEP_ORDER up to and including LAST
  local last=$1 n
  for n in $STEP_ORDER; do
    "step_$n"
    [ "$n" != "$last" ] || break
  done
}

# ---------------------------------------------------------------------------- the final phase
# lexical sort, not -n: `comm` below needs the two lists in `sort`'s own order
sleep_orphans() { ps -o pid= -o ppid= -o command= -A 2>/dev/null | awk '$2 == 1 && /sleep 3600/ && !/awk/ { print $1 }' | sort || true; }

# P14's characterisation pins (plan-rev3 §5.3 P14, landed 05541f1b) fix today's sniffing and
# extraction so the wave's changes are seen; P15–P18 each move their own. The measured diff the
# driver replays predates P14 and carries none of those moves, so on the wave's lock exactly these
# fail — and each must, or the behaviour it pins did not move as measured (rehearsal 2026-10-12).
P14_PINS="security::sanitizer::tests::a_declared_text_upload_with_a_pdf_or_dwg_signature_inside_passes_today P17/P18 renamed
security::sanitizer::tests::the_two_connector_media_uploads_pass_today P17a/P18 renamed
extraction::tests::a_docx_with_bzip2_entries_is_read_by_zip_2 P16 renamed
extraction::tests::an_aes_128_pdf_is_a_retryable_error_today P15 renamed
extraction::tests::xlsx_cell_text_is_pinned_as_calamine_0_26_reads_it P16 renamed
routes::files::tests::a_declared_csv_starting_with_a_dwg_signature_is_accepted P17/P18
routes::files::tests::a_declared_text_file_naming_a_pdf_header_past_offset_0_is_accepted P17/P18"

# Which pins must fail depends on whether the tree under test carries P15–P18 (prep review
# 2026-10-12). They rename the five pins marked `renamed` as they move them, and P17's rule makes
# the two route pins pass under their own names. So: every renamed pin under its old name — the
# branch point — expects all seven; none of them — a tip carrying all four commits — expects none;
# a tree carrying some is neither, and returns 1. Prints the expected P14_PINS lines.
expected_pins() { # expected_pins TREE
  local line name fn renamed=0 present=0
  while IFS= read -r line; do
    case "$line" in *" renamed") ;; *) continue ;; esac
    renamed=$((renamed + 1)); name=${line%% *}; fn=${name##*::}
    if grep -rqs --include='*.rs' "fn ${fn}(" "$1/crates" "$1/apps"; then present=$((present + 1)); fi
  done <<<"$P14_PINS"
  if [ "$present" -eq "$renamed" ]; then printf '%s\n' "$P14_PINS"; return 0; fi
  [ "$present" -eq 0 ] && return 0
  echo "the tree carries part of P15–P18: $present of the $renamed renamed P14 pins are under their old names" >&2
  return 1
}

suite_counts() { # suite_counts LOG -> "passed failed ignored", summed over every `test result:` line
  awk '/^test result: /{ for (i = 1; i <= NF; i++) { if ($(i+1) == "passed;") p += $i; if ($(i+1) == "failed;") f += $i; if ($(i+1) == "ignored;") g += $i } } END { print p + 0, f + 0, g + 0 }' "$1"
}

# suite_verdict LOG RC PINS FLOOR — the full suite's verdict (plan review 2026-10-17, MF-6). PINS:
# the P14_PINS lines expected to fail on this tree; FLOOR: the passed count it may not fall below.
# A test binary that aborts prints no FAILED line and no `test result:` of its own, so the failed
# names alone cannot see it; cargo names every failed binary in `error: test failed, to rerun pass
# <spec>`, and those binaries must be exactly the ones in which an expected pin failed.
suite_verdict() {
  local log=$1 rc=$2 pins=$3 floor=$4 p f g sig other names failed unexpected rerun pinned
  set -- $(suite_counts "$log"); p=$1; f=$2; g=$3
  sig=$(grep -m 1 '(signal: ' "$log" || true)
  [ -z "$sig" ] || die "a test binary was killed by a signal, so its tests did not all run: $(printf '%s' "$sig" | sed 's/^ *//') (log: $log)"
  # rustc's and cargo's own build failures only: the failing pins' captured output is in this log too,
  # and a line of it may start with `error`. (A binary that does not link is also caught below: cargo
  # names no rerun for it, and the passed count falls under the floor.)
  other=$(grep -E '^error(\[E[0-9]+\])?: (could not compile|linking with|failed to run custom build command)|^error\[E[0-9]+\]' "$log" || true)
  [ -z "$other" ] || die "cargo test failed to build: $(first_line "$other") (log: $log)"
  names=$(printf '%s\n' "$pins" | awk 'NF { print $1 }' | sort -u)
  failed=$(sed -n 's/^test \(.*\) \.\.\. FAILED$/\1/p' "$log" | sort -u)
  unexpected=$(comm -23 <(printf '%s\n' "$failed" | grep . || true) <(printf '%s\n' "$names" | grep . || true))
  [ -z "$unexpected" ] || die "the suite failed outside P14's pins: $(printf '%s ' $unexpected)(log: $log)"
  rerun=$(sed -n 's/^error: [a-z]* failed, to rerun pass `\(.*\)`$/\1/p' "$log" | sort -u)
  printf '%s\n' "$names" > "$T/verdict-pins.txt"
  pinned=$(awk 'FNR == NR { if (NF) pin[$1] = 1; next }
                /^ +Running / || /^ +Doc-tests / { hit = 0; next }
                /^test .* \.\.\. FAILED$/ { if ($2 in pin) hit = 1; next }
                /^error: [a-z]+ failed, to rerun pass `/ { if (hit) { s = $0; sub(/^[^`]*`/, "", s); sub(/`$/, "", s); print s } hit = 0 }' \
             "$T/verdict-pins.txt" "$log" | sort -u)
  [ "$rerun" = "$pinned" ] || die "the failed test binaries are not exactly those of P14's pins: cargo names [$(oneline "$rerun")], the pins failed in [$(oneline "$pinned")] — a binary that crashed or did not link ran none of its tests (log: $log)"
  if [ -z "$names" ] && [ "$rc" -ne 0 ]; then die "no test is expected to fail on this tree, and cargo test exited rc=$rc (log: $log)"; fi
  case "$rc" in 0|101) ;; *) die "cargo test exited rc=$rc, neither a pass nor a test failure (log: $log)" ;; esac
  [ "$rc" -eq 0 ] || [ -n "$rerun" ] || die "cargo test exited rc=$rc and named no failed test binary (log: $log)"
  [ "$p" -ge "$floor" ] || die "the suite passed $p tests, below the floor of $floor (--min-passed): a test binary that did not run, or tests lost (log: $log)"
  return 0
}

final_tests() {
  CURRENT_STEP="final-tests"
  say ""; say "== final: the full suite   ($(now))"
  local store="$T/test-store" home="$T/test-home" rustup_home="" cargo_home="" before="" after="" new="" p=0 f=0 g=0 test_rc
  rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"; cargo_home="${CARGO_HOME:-$HOME/.cargo}"
  if live; then
    mkdir -p "$store/config" "$home"; before=$(sleep_orphans)
    # Two tests need a git checkout around them (memory/scope_context.rs:174's precondition and
    # request_workspace_threading); `git archive` exports none (rehearsal 2026-10-12).
    git -C "$T/src" init -q || die "git init in the export failed"
  else
    plan "    git -C \$T/src init -q   (two tests need a git checkout)"
  fi
  # cargo_cmd's environment (the build dir pinned to the target dir, MF-5) plus the test sandbox
  run_logged_rc 90-test nice -n 10 env CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0 "CARGO_BUILD_JOBS=$JOBS" \
    "CARGO_TARGET_DIR=$T/target" "CARGO_BUILD_BUILD_DIR=$T/target" "OPENALPACA_HOME_STORE=$store" \
    "OPENALPACA_CONFIG_DIR=$store/config" "HOME=$home" "RUSTUP_HOME=$rustup_home" "CARGO_HOME=$cargo_home" \
    cargo "+$TOOLCHAIN" test --workspace --exclude openalpaca_gui --no-fail-fast
  test_rc=$LAST_RC
  if ! live; then
    plan "    verdict: no '(signal:' line; no build error; no failure outside the pins expected on this tree; the binaries cargo names for a rerun exactly those in which a pin failed; no pin expected ⇒ rc 0; passed ≥ --min-passed (${MIN_PASSED:-?}); the lock still step 8's"
    return 0
  fi
  set -- $(suite_counts "$T/logs/90-test.log")
  p=$1; f=$2; g=$3
  TEST_LINE="$p passed / $f failed / $g ignored (rc=$test_rc; floor $MIN_PASSED)"
  say "    cargo test: $TEST_LINE"
  grep -E '^test .* \.\.\. FAILED$|^error: (test|doctest) failed' "$T/logs/90-test.log" | sed 's/^/      /' || true
  after=$(sleep_orphans)
  new=$(comm -13 <(printf '%s\n' $before) <(printf '%s\n' $after) | grep . || true)
  if [ -n "$new" ]; then
    say "    orphaned 'sleep 3600' children left by the daemon's tests (ppid 1, started in this run), killed: $(printf '%s ' $new)"
    kill $new 2>/dev/null || true
  fi
  local failed missing name pins
  pins=$(expected_pins "$T/src") || die "G-base runs on the branch point or on a tip carrying all of P15–P18, not between them"
  suite_verdict "$T/logs/90-test.log" "$test_rc" "$pins" "$MIN_PASSED"
  failed=$(sed -n 's/^test \(.*\) \.\.\. FAILED$/\1/p' "$T/logs/90-test.log" | sort -u)
  missing=$(comm -13 <(printf '%s\n' "$failed" | grep . || true) <(printf '%s\n' "$pins" | awk '{print $1}' | sort -u))
  say "    P14's pins expected to fail on this tree: $(printf '%s\n' "$pins" | grep -c . || true) of $(printf '%s\n' "$P14_PINS" | grep -c .) (none on a tip carrying P15–P18); failing: $(printf '%s\n' "$failed" | grep -c . || true); no crashed binary; passed $p ≥ $MIN_PASSED"
  if [ -n "$missing" ]; then
    for name in $missing; do say "    NOTE: P14 pin did not fail — the behaviour it pins did not move as measured: $name ($(printf '%s\n' "$pins" | awk -v n="$name" '$1 == n {print $2}'))"; done
    [ "$STRICT" -eq 0 ] || die "under --strict every P14 pin must fail on the wave's lock"
  fi
  # the suite runs without --locked (G-check's rule), so hold it to step 8's lock as G-GUI is held to its own
  cmp -s Cargo.lock "$T/locks/step-8.lock" || die "the suite changed Cargo.lock after step 8 (compare $T/locks/step-8.lock)"
}

final_docs_check() {
  CURRENT_STEP="final-docs"
  say ""; say "== final: python3 -I scripts/gen_api_docs.py --check  (expected: exactly docs/api/crates/openalpaca_mcp.md stale, P19/P21)"
  run_logged_rc 91-gen-api-docs python3 -I scripts/gen_api_docs.py --check
  live || return 0
  local changed
  changed=$(sed -n 's/^- //p' "$T/logs/91-gen-api-docs.log" | sort | tr '\n' ' ')
  say "    rc=$LAST_RC stale: ${changed:-none}"
  [ "$LAST_RC" -eq 1 ] && [ "$changed" = "docs/api/crates/openalpaca_mcp.md " ] \
    || die "gen_api_docs.py --check: expected rc 1 with exactly docs/api/crates/openalpaca_mcp.md, got rc $LAST_RC with: ${changed:-none}"
}

js_edits() {
  local f=apps/openalpaca-gui/package.json on ov nn nv c end
  while read -r on ov nn nv c; do
    [ -n "$on" ] || continue
    nv=$(js_pin_version "$nn" "$nv")
    end=""; [ "$c" = "," ] && end=","
    edit "$f" "    \"$on\": \"$ov\"$end" "    \"$nn\": \"$nv\"$end"
  done <<EOF
$JS_PINS
EOF
  edit "$f" '    "@tauri-apps/api": "2.11.1",' "    \"@tauri-apps/api\": \"$JS_API\","
  edit "$f" '    "@tauri-apps/plugin-dialog": "2.7.3",' "    \"@tauri-apps/plugin-dialog\": \"$DIALOG\","
  edit "$f" '    "@tauri-apps/cli": "2.11.4",' "    \"@tauri-apps/cli\": \"$JS_CLI\","
  edit apps/openalpaca-gui/vite.config.ts 'import react from "@vitejs/plugin-react-swc";' 'import react from "@vitejs/plugin-react";'
}

js_pin() { # js_pin PACKAGE — the version apps/openalpaca-gui/package.json pins PACKAGE at
  sed -n "s|^    \"$1\": \"\(.*\)\",\{0,1\}\$|\1|p" apps/openalpaca-gui/package.json
}

# home_writes — what Bun or npm wrote under the real HOME since the run began (tasks/lessons.md,
# 2026-10: every cache a confined tool keeps, then a `find` written for bash). Prints the files.
home_writes() {
  local d
  for d in "$HOME/.bun/install/cache" "$HOME/Library/Caches/bun" "$HOME/.cache/bun" "$HOME/.npm/_logs" "$HOME/.npm/_cacache" \
           "$HOME/Library/Application Support/vitest"; do
    [ -d "$d" ] || continue
    find "$d" -newer "$T/.started" -type f 2>/dev/null | head -n 5 || true
  done
}

# bun_lock_dates [LOG] — the cut-day compatibility report's C5: bunfig.toml's minimumReleaseAge
# counts 7 days back from the install, DU-23 from the cut instant, and every transitive package
# floats in its range, so an install after the cut can take a transitive release published after
# the max date. Every name@version the new bun.lock holds — transitive and platform packages
# included — is dated on npm against MAX_DATE (lockdates.py --bun-lock: the --pins npm verdict).
# A young entry ends the run: there is no JS step-back; a recorded DU-23 (c) waiver admits it.
bun_lock_dates() {
  local log=${1:-92b-bun-lock-dates} young
  [ "$MODE" = check ] && return 0
  if ! live; then
    plan "    freshness of the new bun.lock (C5): every name@version it holds, transitive and platform packages included, published on npm at or before $MAX_DATE, or waived (lockdates.py --bun-lock)   > logs/$log.log"
    return 0
  fi
  run_logged_rc "$log" "${LOCKDATES[@]}" --bun-lock bun.lock "${REGFLAGS[@]}"
  case "$LAST_RC" in
    0) ;;
    1) young=$(grep -c '^  TOO YOUNG ' "$T/logs/$log.log" || true)
       grep '^  TOO YOUNG ' "$T/logs/$log.log" >&2 || true
       die "final: the new bun.lock holds $young version(s) published after $MAX_DATE, younger than 7 days at the cut (logs/$log.log): bunfig.toml's minimumReleaseAge counted from the install admitted them, DU-23 counts from the cut — there is no JS step-back; a recorded DU-23 (c) waiver in --waivers, or a run nearer the cut instant (C5)" ;;
    *) tail -n 5 "$T/logs/$log.log" >&2 || true
       die "final: no verdict on the new bun.lock's dates (rc=$LAST_RC; a failed lookup, or a lock this check cannot read, is no verdict; logs/$log.log)" ;;
  esac
  say "    bun.lock freshness ($MAX_DATE): $(oneline "$(grep -m 1 '^bun.lock ' "$T/logs/$log.log" | sed 's/^bun.lock [^:]*: //')"); $(grep -m 1 '^pins (npm):' "$T/logs/$log.log" | sed 's/^pins (npm): max date [^;]*; //') — each published at or before $MAX_DATE, or waived"
}

final_js() {
  CURRENT_STEP="final-js"
  say ""; say "== final: the P31a+P31+P32+P33 JS set, a fresh root bun.lock dated against the cut, the four bun gates   ($(now))"
  js_edits
  # DU-26's pair, read back from package.json against the final lock (step 8's)
  if live; then pair_check "final: package.json against Cargo.lock" "$(js_pin @tauri-apps/api)" "$(js_pin @tauri-apps/plugin-dialog)"
  else pair_check "final: package.json against Cargo.lock" "$JS_API" "$DIALOG"; fi
  local -a BUNENV
  BUNENV=(env "BUN_INSTALL=$T/bun-home" "BUN_INSTALL_CACHE_DIR=$T/bun-cache" \
    "BUN_RUNTIME_TRANSPILER_CACHE_PATH=$T/bun-cache/transpiler" "npm_config_cache=$T/npm-cache" \
    "npm_config_logs_dir=$T/npm-logs" "OPENALPACA_HOME_STORE=$T/test-store" \
    "OPENALPACA_CONFIG_DIR=$T/test-store/config" "XDG_CACHE_HOME=$T/xdg-cache" "HOME=$T/bun-gate-home" \
    NO_COLOR=1 FORCE_COLOR=0 CI=true)
  # HOME in scratch: vitest writes $HOME/Library/Application Support/vitest/.vitest-secret-token,
  # which no Bun variable redirects (the D1 step's combined-tree proof, 2026-10-17). CI=true, not 1:
  # tauri-cli 2.12.x reads CI as its --ci flag, which accepts only true or false.
  if live; then
    mkdir -p "$T/bun-home" "$T/bun-cache" "$T/npm-cache" "$T/npm-logs" "$T/test-store/config" "$T/bun-gate-home"
    rm -f bun.lock; say "    removed the root bun.lock (P31: written from scratch by the pinned Bun)"
  else
    plan "    rm bun.lock   (the root lock, re-resolved from scratch)"
  fi
  run_logged 92-bun-install "${BUNENV[@]}" "$BUN" install
  live && say "    bun.lock: $(sed -n 's/^ *"lockfileVersion": *\([0-9]*\),*$/lockfileVersion \1/p' bun.lock | head -n 1)"
  bun_lock_dates
  cd apps/openalpaca-gui
  run_logged 93-bun-check "${BUNENV[@]}" "$BUN" run check
  run_logged 94-bun-test "${BUNENV[@]}" "$BUN" run test
  run_logged 95-bun-format-check "${BUNENV[@]}" "$BUN" run format:check
  run_logged 96-bun-build "${BUNENV[@]}" "$BUN" run build
  cd "$T/src"
  if live; then
    VITEST_LINE=$(grep -E '^ *(Test Files|Tests) ' "$T/logs/94-bun-test.log" | sed 's/^ *//' | tr '\n' ';' | sed 's/;$//; s/;/; /g')
    say "    vitest: ${VITEST_LINE:-summary lines not found in logs/94-bun-test.log}"
    local w
    w=$(home_writes)
    [ -z "$w" ] || die "the bun gates wrote under the real HOME (a cache the confinement missed): $(oneline "$w")"
    say "    nothing new under the real HOME's Bun and npm caches since the run began"
  else
    plan "    then: nothing new under ~/.bun/install/cache, ~/Library/Caches/bun, ~/.cache/bun, ~/.npm/_logs, ~/.npm/_cacache since the run began"
  fi
}

final_gui() {
  CURRENT_STEP="final-gui"
  say ""; say "== final: G-GUI — cargo check and clippy of openalpaca_gui into target-gui   ($(now))"
  local triple sidecar
  if live; then triple=$(rustc "+$TOOLCHAIN" -vV | sed -n 's/^host: //p'); else triple='<host triple>'; fi
  sidecar="apps/openalpaca-gui/src-tauri/bin/openalpacad-$triple"
  if live; then
    mkdir -p apps/openalpaca-gui/src-tauri/bin apps/openalpaca-gui/dist
    : > "$sidecar"
    say "    placeholder sidecar: $sidecar (empty; tauri-build errors when it is absent)"
    cp Cargo.lock "$T/locks/before-gui.lock"
  else
    plan "    : > $sidecar ; mkdir -p apps/openalpaca-gui/dist"
  fi
  run_logged 97-gui-check "${CARGO_GUI[@]}" check -p openalpaca_gui --locked
  run_logged 98-gui-clippy "${CARGO_GUI[@]}" clippy -p openalpaca_gui --all-targets --locked
  if live; then
    cmp -s Cargo.lock "$T/locks/before-gui.lock" || die "G-GUI changed the lock although it ran --locked"
    GUI_LINE="check $(count_diag "$T/logs/97-gui-check.log"); clippy $(count_diag "$T/logs/98-gui-clippy.log")"
    say "    G-GUI: $GUI_LINE"
    rm -f "$sidecar"
  fi
}

# archive_evidence — A1-17: a real run's logs and locks (the lock differences, their dates, the
# step-backs, the registry facts the gate decided on, every per-step lock) as one tarball beside
# them, written at exit whatever the outcome, and copied into --archive-to when given. Nothing else
# of $T goes in: not the export, not a target dir.
archive_evidence() {
  [ "$ARCHIVE" -eq 1 ] && [ -n "$T" ] && [ -d "$T/logs" ] || return 0
  local name dest
  name="evidence-$RUN_DATE-gbase.tar.gz"
  tar -czf "$T/$name" -C "$T" logs locks 2>/dev/null || { echo "gbase: could not write the evidence archive $T/$name" >&2; return 0; }
  echo "gbase: evidence archive (logs/ and locks/ only): $T/$name" >&2
  if [ -n "$ARCHIVE_TO" ]; then
    dest="$ARCHIVE_TO/$name"
    [ ! -e "$dest" ] || dest="$ARCHIVE_TO/evidence-$RUN_DATE-$(date -u +%H%M%SZ)-gbase.tar.gz"
    cp "$T/$name" "$dest" && echo "gbase: copied to $dest" >&2 || echo "gbase: could not copy it to $ARCHIVE_TO" >&2
  fi
  return 0
}

summary() {
  local total=$(( LOCK_UNEXPECTED + LOCK_EXPECTED ))
  if [ "$total" -eq 0 ]; then LOCK_LINE="every compared lock equal to its shipped snapshot"
  else LOCK_LINE="$LOCK_UNEXPECTED unexpected, $LOCK_EXPECTED expected (rmcp 3.5.1, DU-25 (v4); serde-saphyr 2.0.0, D4 (a)) — $T/logs/lock-differences.txt"; fi
  if [ "$DATES" -eq 1 ] && [ "$total" -gt 0 ]; then
    say ""; say "== release dates of the versions the differences introduced (crates.io; User-Agent openalpaca-deps-check)"
    python3 -I "$HERE/lockdates.py" < "$T/logs/lock-differences.txt" > "$T/logs/lock-dates.txt" 2>&1 || true
    sed 's/^/    /' "$T/logs/lock-dates.txt"
  fi
  say ""
  say "RESULT ($(now)): cut $NOT_BEFORE, freshness max date $MAX_DATE, $STEPBACKS_TOTAL step-back(s)$( [ "$STEPBACKS_TOTAL" -eq 0 ] || printf ' (%s)' "$STEPBACKS_FILE"); lock differences: $LOCK_LINE; cargo test: $TEST_LINE; vitest: $VITEST_LINE; G-GUI: $GUI_LINE; logs: $T/logs; evidence: $T/evidence-$RUN_DATE-gbase.tar.gz (at exit)"
}

# ---------------------------------------------------------------------------- pre-flight
# preflight_pins — MF-2 item 4: every pin the run takes, against its registry's dates, before cargo.
preflight_pins() {
  local name row on ov nn nv c pins="" js=""
  for name in tauri $TAURI_INTERNALS tauri-plugin-dialog; do pins="$pins $name@$(tauri_expected "$name")"; done
  run_logged_rc preflight-pins-crates "${LOCKDATES[@]}" --pins crates "${REGFLAGS[@]}" $pins
  case "$LAST_RC" in
    0) ;;
    1) grep '^  TOO YOUNG ' "$T/logs/preflight-pins-crates.log" >&2 || true; die "pre-flight: a Tauri crate the run would take is younger than 7 days at the cut (published after $MAX_DATE; logs/preflight-pins-crates.log)" ;;
    *) die "pre-flight: no verdict on the Tauri crates' dates (rc=$LAST_RC; logs/preflight-pins-crates.log)" ;;
  esac
  [ "$STEPS" = all ] || return 0
  js="@tauri-apps/api@$JS_API @tauri-apps/plugin-dialog@$DIALOG @tauri-apps/cli@$JS_CLI"
  while read -r on ov nn nv c; do
    [ -n "$on" ] || continue
    js="$js $nn@$(js_pin_version "$nn" "$nv")"
  done <<EOF
$JS_PINS
EOF
  # bunfig.toml's minimumReleaseAge is measured from the install, not from the cut: this is the cut's
  # check of the pins, and bun_lock_dates is the cut's check of every entry the install writes (C5)
  run_logged_rc preflight-pins-npm "${LOCKDATES[@]}" --pins npm "${REGFLAGS[@]}" $js
  case "$LAST_RC" in
    0) ;;
    1) grep '^  TOO YOUNG ' "$T/logs/preflight-pins-npm.log" >&2 || true; die "pre-flight: a JS pin (or a platform binary of @tauri-apps/cli) is younger than 7 days at the cut (published after $MAX_DATE; logs/preflight-pins-npm.log)" ;;
    *) die "pre-flight: no verdict on the JS pins' dates (rc=$LAST_RC; logs/preflight-pins-npm.log)" ;;
  esac
}

preflight() {
  CURRENT_STEP="preflight"
  need git; need tar; need perl; need python3; need awk; need cargo
  cargo "+$TOOLCHAIN" --version > "$T/logs/preflight-cargo-version.log" 2>&1 \
    || { cat "$T/logs/preflight-cargo-version.log" >&2; echo "gbase: cargo +$TOOLCHAIN is not installed (this script installs nothing)" >&2; exit 2; }
  say "cargo: $(cat "$T/logs/preflight-cargo-version.log")"
  if [ "$STEPS" = all ] && [ "$EDITS_ONLY" -eq 0 ]; then
    [ -n "$BUN" ] || { echo "gbase: --bun <path> is required when the final phase runs (--steps all)" >&2; exit 2; }
    [ -x "$BUN" ] || { echo "gbase: --bun $BUN is not an executable" >&2; exit 2; }
    say "bun: $("$BUN" --version 2>&1) at $BUN"
    [ -n "$MIN_PASSED" ] || { echo "gbase: --min-passed <N> is required when the final phase runs (--steps all): the floor of the suite's passed count (README.md, \"The final phase\")" >&2; exit 2; }
  fi
  local f
  for f in lockdiff.py lockdates.py patches/step-4-secrets.patch patches/step-6-rmcp.patch \
           patches/step-8-config.patch locks/recut-lock-00-head.lock locks/recut-lock-01-toolchain.lock \
           locks/d1-lock-01b-pair.lock locks/recut-lock-02-update.lock locks/recut-lock-03-storage.lock \
           locks/recut-lock-04-secrets.lock locks/recut-lock-05-documents.lock locks/recut-lock-06-rmcp.lock \
           locks/recut-lock-07-web.lock locks/recut-lock-99-final.lock; do
    [ -f "$HERE/$f" ] || die "pre-flight: missing shipped file $HERE/$f"
  done

  # the export
  BASE_SHA=$(git -C "$REPO" rev-parse --verify "$BASE^{commit}") || die "pre-flight: cannot resolve --base $BASE in $REPO"
  mkdir -p "$T/src"
  git -C "$REPO" archive --format=tar "$BASE_SHA" | tar -x -C "$T/src"
  say "exported $BASE_SHA ($(git -C "$REPO" log -1 --format='%h %s' "$BASE_SHA")) to $T/src"
  cd "$T/src"
  cp Cargo.lock "$T/locks/step-0-base.lock"
  ( STRICT=0; compare_lock "base-vs-measured-head" "$HERE/locks/recut-lock-00-head.lock" "$T/locks/step-0-base.lock" ) >/dev/null
  if cmp -s Cargo.lock "$HERE/locks/recut-lock-00-head.lock"; then
    say "the base's Cargo.lock is byte-identical to the measured head's (7295f91b)"
  elif [ "$(head -n 1 "$T/logs/lockdiff-base-vs-measured-head.txt" | awk '{ print $2 + $4 + $6 }')" = 0 ]; then
    say "NOTE: the base's Cargo.lock holds the measured head's (name, version) set; its bytes differ (a dependency edge — logs/lockdiff-base-vs-measured-head.txt, and \`diff\` it with locks/recut-lock-00-head.lock)"
  else
    say "NOTE: the base's Cargo.lock differs from the measured head's — the per-step comparisons will show it; see logs/lock-differences.txt"
  fi
  LOCK_UNEXPECTED=0; LOCK_EXPECTED=0   # the base comparison is information, not a finding

  # every anchor and every patch against the pristine export, before any cargo runs
  local saved_mode=$MODE
  MODE=check
  run_steps 8
  # the JS anchors bind only a run that reaches the final phase, as --bun does
  if [ "$STEPS" = all ] && [ "$EDITS_ONLY" -eq 0 ]; then js_edits; fi
  MODE=$saved_mode
  say "pre-flight: $ANCHORS_CHECKED anchors match exactly once; 3 patches apply to the base"
  # last, after the export: the registries (network) only once the base is known to be good
  if [ "$REAL" -eq 1 ]; then
    preflight_pins
    say "pre-flight: every pin the run takes was published at or before $MAX_DATE (cut $NOT_BEFORE)"
  else
    say "pre-flight: the pins' registry dates are checked by a real run only (max date $MAX_DATE, cut $NOT_BEFORE)"
  fi
  CURRENT_STEP="setup"
}

# ---------------------------------------------------------------------------- the self-test
self_test() {
  CURRENT_STEP="self-test"
  need perl; need python3; need cargo; need tar
  cargo "+$TOOLCHAIN" --version > "$T/logs/selftest-cargo-version.log" 2>&1 \
    || { cat "$T/logs/selftest-cargo-version.log" >&2; echo "gbase: cargo +$TOOLCHAIN is not installed" >&2; exit 2; }
  say "self-test with $(cat "$T/logs/selftest-cargo-version.log")"
  local toy="$T/toy" pass=0 fail=0 rc
  mkdir -p "$toy/a/src" "$toy/b/src"
  cat > "$toy/Cargo.toml" <<'EOF'
[workspace]
members = ["a", "b"]
resolver = "2"
EOF
  cat > "$toy/a/Cargo.toml" <<'EOF'
[package]
name = "a"
version = "0.1.0"
edition = "2021"
EOF
  echo 'pub fn one() -> u32 { 1 }' > "$toy/a/src/lib.rs"
  cat > "$toy/b/Cargo.toml" <<'EOF'
[package]
name = "b"
version = "0.1.0"
edition = "2021"

[dependencies]
a = { path = "../a" }
EOF
  echo 'fn main() { println!("{}", a::one()); }' > "$toy/b/src/main.rs"
  cd "$toy"
  cargo_cmd "$T/target"
  CARGO=("${CARGO_CMD[@]}")

  verdict() { # verdict NAME COND-RC DESCRIPTION
    if [ "$2" -eq 0 ]; then pass=$((pass + 1)); say "  PASS $1: $3"; else fail=$((fail + 1)); say "  FAIL $1: $3"; fi
  }

  # control: a passing cargo command passes through run_logged
  rc=0; run_logged selftest-00-control-check "${CARGO[@]}" check --offline || rc=$?
  verdict control-pass $rc "a passing cargo check returns through run_logged (rc=$rc)"
  cp Cargo.lock "$T/locks/selftest-lock-A.lock"

  # (i) a failing cargo command stops the driver: non-zero exit, the sentinel after it never runs
  echo 'fn main() { let _x: u32 = "no"; }' > b/src/main.rs
  rc=0
  ( run_logged selftest-01-failing-check "${CARGO[@]}" check --offline; touch "$T/must-not-exist-1" ) \
    > "$T/logs/selftest-01-driver.log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ ! -e "$T/must-not-exist-1" ] && grep -q 'command failed (rc=101)' "$T/logs/selftest-01-driver.log"; then
    verdict i 0 "a failing cargo check stopped the driver (rc=$rc, nothing after it ran; logs/selftest-01-driver.log)"
  else
    verdict i 1 "rc=$rc, sentinel present: $([ -e "$T/must-not-exist-1" ] && echo yes || echo no)"
  fi
  echo 'fn main() { println!("{}", a::one()); }' > b/src/main.rs

  # (ii) an anchor that matches zero or two times stops the driver and changes nothing
  printf 'alpha\nbeta\nbeta\n' > anchors.txt; cp anchors.txt anchors.orig
  rc=0
  ( edit anchors.txt gamma delta; touch "$T/must-not-exist-2" ) > "$T/logs/selftest-02a-anchor-zero.log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ ! -e "$T/must-not-exist-2" ] && cmp -s anchors.txt anchors.orig && grep -q 'matched 0 times' "$T/logs/selftest-02a-anchor-zero.log"; then
    verdict ii-zero 0 "an anchor matching 0 times stopped the driver; the file is unchanged (logs/selftest-02a-anchor-zero.log)"
  else
    verdict ii-zero 1 "rc=$rc"
  fi
  rc=0
  ( edit anchors.txt beta delta; touch "$T/must-not-exist-3" ) > "$T/logs/selftest-02b-anchor-twice.log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ ! -e "$T/must-not-exist-3" ] && cmp -s anchors.txt anchors.orig && grep -q 'matched 2 times' "$T/logs/selftest-02b-anchor-twice.log"; then
    verdict ii-twice 0 "an anchor matching 2 times stopped the driver; the file is unchanged (logs/selftest-02b-anchor-twice.log)"
  else
    verdict ii-twice 1 "rc=$rc"
  fi
  rc=0; ( edit anchors.txt alpha omega ) > "$T/logs/selftest-02c-anchor-once.log" 2>&1 || rc=$?
  if [ "$rc" -eq 0 ] && [ "$(head -n 1 anchors.txt)" = omega ] && [ "$(grep -c beta anchors.txt)" = 2 ]; then
    verdict ii-control 0 "an anchor matching once is replaced, the other lines untouched"
  else
    verdict ii-control 1 "rc=$rc, first line: $(head -n 1 anchors.txt)"
  fi

  # (iii) the lock comparison prints a difference; under --strict it fails
  edit a/Cargo.toml 'version = "0.1.0"' 'version = "0.1.1"' > /dev/null
  run_logged selftest-03-generate-lockfile "${CARGO[@]}" generate-lockfile --offline
  cp Cargo.lock "$T/locks/selftest-lock-B.lock"
  rc=0; STRICT=0
  compare_lock "selftest-lenient" "$T/locks/selftest-lock-A.lock" "$T/locks/selftest-lock-B.lock" > "$T/logs/selftest-03a-lockdiff-lenient.log" 2>&1 || rc=$?
  if [ "$rc" -eq 0 ] && grep -q 'a: 0.1.0 -> 0.1.1' "$T/logs/selftest-03a-lockdiff-lenient.log"; then
    verdict iii-print 0 "the comparison printed 'a: 0.1.0 -> 0.1.1' and went on without --strict (logs/selftest-03a-lockdiff-lenient.log)"
  else
    verdict iii-print 1 "rc=$rc; see logs/selftest-03a-lockdiff-lenient.log"
  fi
  rc=0
  ( STRICT=1; compare_lock "selftest-strict" "$T/locks/selftest-lock-A.lock" "$T/locks/selftest-lock-B.lock"; touch "$T/must-not-exist-4" ) \
    > "$T/logs/selftest-03b-lockdiff-strict.log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ ! -e "$T/must-not-exist-4" ] && grep -q 'unexpected lock difference' "$T/logs/selftest-03b-lockdiff-strict.log"; then
    verdict iii-strict 0 "under --strict the same difference stopped the driver (rc=$rc; logs/selftest-03b-lockdiff-strict.log)"
  else
    verdict iii-strict 1 "rc=$rc"
  fi
  rc=0
  compare_lock "selftest-equal" "$T/locks/selftest-lock-A.lock" "$T/locks/selftest-lock-A.lock" > "$T/logs/selftest-03c-lockdiff-equal.log" 2>&1 || rc=$?
  if [ "$rc" -eq 0 ] && grep -q 'byte-identical' "$T/logs/selftest-03c-lockdiff-equal.log"; then
    verdict iii-equal 0 "equal locks are reported equal"
  else
    verdict iii-equal 1 "rc=$rc"
  fi

  # (iv) an exact expectation (name:OLD->NEW, the form of step 6's rmcp label) covers exactly that
  # move: under --strict the labelled difference passes, the same name moving elsewhere stops it
  rc=0
  ( STRICT=1; compare_lock "selftest-exact" "$T/locks/selftest-lock-A.lock" "$T/locks/selftest-lock-B.lock" \
      --expected 'toy=a:0.1.0->0.1.1' ) > "$T/logs/selftest-04a-exact-label.log" 2>&1 || rc=$?
  if [ "$rc" -eq 0 ] && grep -q 'a: 0.1.0 -> 0.1.1  \[toy\]' "$T/logs/selftest-04a-exact-label.log" && grep -q 'unexpected: 0' "$T/logs/selftest-04a-exact-label.log"; then
    verdict iv-exact 0 "'a:0.1.0->0.1.1' labels that move expected, and --strict passes (logs/selftest-04a-exact-label.log)"
  else
    verdict iv-exact 1 "rc=$rc; see logs/selftest-04a-exact-label.log"
  fi
  rc=0
  ( STRICT=1; compare_lock "selftest-exact-other" "$T/locks/selftest-lock-A.lock" "$T/locks/selftest-lock-B.lock" \
      --expected 'toy=a:0.1.0->0.1.2'; touch "$T/must-not-exist-5" ) > "$T/logs/selftest-04b-exact-other.log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ ! -e "$T/must-not-exist-5" ] && grep -q 'unexpected lock difference' "$T/logs/selftest-04b-exact-other.log"; then
    verdict iv-exact-other 0 "'a:0.1.0->0.1.2' does not cover a move to 0.1.1: --strict stopped the driver (rc=$rc)"
  else
    verdict iv-exact-other 1 "rc=$rc; see logs/selftest-04b-exact-other.log"
  fi

  # (v) expect_lock: the version a step asserts (step 1b's pair, step 6's rmcp) — a match passes,
  # a mismatch stops the driver with nothing after it
  rc=0; ( expect_lock a '^0[.]1[.]1$' "selftest" ) > "$T/logs/selftest-05a-expect-match.log" 2>&1 || rc=$?
  verdict v-match "$rc" "expect_lock a ^0.1.1\$ passes on a lock holding a 0.1.1 (rc=$rc)"
  rc=0
  ( expect_lock a '^0[.]2[.]' "selftest"; touch "$T/must-not-exist-6" ) > "$T/logs/selftest-05b-expect-mismatch.log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ ! -e "$T/must-not-exist-6" ] && grep -q "holds a at '0.1.1 '" "$T/logs/selftest-05b-expect-mismatch.log"; then
    verdict v-mismatch 0 "expect_lock a ^0.2. stopped the driver on a 0.1.1 lock (rc=$rc; logs/selftest-05b-expect-mismatch.log)"
  else
    verdict v-mismatch 1 "rc=$rc; see logs/selftest-05b-expect-mismatch.log"
  fi

  # (vi) pair_check (DU-26): on a lock holding tauri 2.12.3 and tauri-plugin-dialog 2.8.2, the
  # api 2.12.x / dialog 2.8.2 pair passes; a new api minor, or a dialog one patch off, stops it
  mkdir -p "$T/toy-pair"
  printf '[[package]]\nname = "tauri"\nversion = "2.12.3"\n\n[[package]]\nname = "tauri-plugin-dialog"\nversion = "2.8.2"\n' > "$T/toy-pair/Cargo.lock"
  rc=0; ( cd "$T/toy-pair" && pair_check selftest 2.12.9 2.8.2 ) > "$T/logs/selftest-06a-pair-holds.log" 2>&1 || rc=$?
  verdict vi-holds "$rc" "tauri 2.12.3 ~ api 2.12.9, dialog 2.8.2 = 2.8.2: the pair holds (rc=$rc)"
  rc=0
  ( cd "$T/toy-pair" && { pair_check selftest 2.13.0 2.8.2; touch "$T/must-not-exist-7"; } ) > "$T/logs/selftest-06b-pair-minor.log" 2>&1 || rc=$?
  local rc2=0
  ( cd "$T/toy-pair" && { pair_check selftest 2.12.3 2.8.1; touch "$T/must-not-exist-8"; } ) > "$T/logs/selftest-06c-pair-dialog.log" 2>&1 || rc2=$?
  if [ "$rc" -ne 0 ] && [ "$rc2" -ne 0 ] && [ ! -e "$T/must-not-exist-7" ] && [ ! -e "$T/must-not-exist-8" ] \
     && grep -q 'pair is broken' "$T/logs/selftest-06b-pair-minor.log" && grep -q 'pair is broken' "$T/logs/selftest-06c-pair-dialog.log"; then
    verdict vi-broken 0 "api 2.13.0 against tauri 2.12.3, and dialog 2.8.1 against 2.8.2, each stopped the driver (rc=$rc, $rc2)"
  else
    verdict vi-broken 1 "rc=$rc, $rc2; see logs/selftest-06b-pair-minor.log, logs/selftest-06c-pair-dialog.log"
  fi

  # (vii) a lock comparison lockdiff.py could not make stops the driver, --strict or not: a
  # non-lock file — bytes that are not UTF-8, or UTF-8 text with no [[package]] — refused by
  # lockdiff.py (exit 2); and a lockdiff.py that dies before its summary (a syntax error: exit 1
  # and Python's error report, which compare_lock's field-wise parse would read as "the same
  # set"), refused by compare_lock's summary-line check
  printf '\377\376not a lock\n' > "$T/toy-not-utf8.lock"
  printf '# not a Cargo.lock\nhello = "world"\n' > "$T/toy-no-package.lock"
  rc=0
  ( STRICT=0; compare_lock "selftest-not-utf8" "$T/locks/selftest-lock-A.lock" "$T/toy-not-utf8.lock"; touch "$T/must-not-exist-9" ) \
    > "$T/logs/selftest-07a-not-utf8.log" 2>&1 || rc=$?
  rc2=0
  ( STRICT=0; compare_lock "selftest-no-package" "$T/locks/selftest-lock-A.lock" "$T/toy-no-package.lock"; touch "$T/must-not-exist-10" ) \
    > "$T/logs/selftest-07b-no-package.log" 2>&1 || rc2=$?
  if [ "$rc" -ne 0 ] && [ "$rc2" -ne 0 ] && [ ! -e "$T/must-not-exist-9" ] && [ ! -e "$T/must-not-exist-10" ] \
     && grep -q 'FAIL: lockdiff.py' "$T/logs/selftest-07a-not-utf8.log" && grep -q 'FAIL: lockdiff.py' "$T/logs/selftest-07b-no-package.log"; then
    verdict vii-nonlock 0 "a non-UTF-8 file and a UTF-8 file with no [[package]], each compared with a lock, stopped the driver (rc=$rc, $rc2; logs/selftest-07a-not-utf8.log, logs/selftest-07b-no-package.log)"
  else
    verdict vii-nonlock 1 "rc=$rc, $rc2; see logs/selftest-07a-not-utf8.log, logs/selftest-07b-no-package.log"
  fi
  mkdir -p "$T/toy-broken-tools"
  printf 'def main(:\n' > "$T/toy-broken-tools/lockdiff.py"
  rc=0
  ( HERE="$T/toy-broken-tools"; STRICT=0; compare_lock "selftest-no-summary" "$T/locks/selftest-lock-A.lock" "$T/locks/selftest-lock-B.lock"; touch "$T/must-not-exist-11" ) \
    > "$T/logs/selftest-07c-no-summary.log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ ! -e "$T/must-not-exist-11" ] && grep -q 'lockdiff.py gave no summary (rc=1)' "$T/logs/selftest-07c-no-summary.log"; then
    verdict vii-nosummary 0 "a lockdiff.py that dies before its summary (rc 1, Python's error report) stopped the driver (rc=$rc; logs/selftest-07c-no-summary.log)"
  else
    verdict vii-nosummary 1 "rc=$rc; see logs/selftest-07c-no-summary.log"
  fi

  # (viii) the pins expected to fail follow P15–P18: a toy branch point (every renamed pin under
  # its old name) expects all seven; a toy tip (the renamed pins gone, the two route pins kept
  # under their names, as P17 leaves them) expects none; a toy tree between them is refused
  local t old fn got
  for t in branch tip between; do mkdir -p "$T/toy-pins-$t/crates/x/src" "$T/toy-pins-$t/apps/y/src"; done
  old=$(printf '%s\n' "$P14_PINS" | awk '$3 == "renamed" {print $1}')
  for fn in $old; do printf '#[test]\nfn %s() {}\n' "${fn##*::}" >> "$T/toy-pins-branch/crates/x/src/pins.rs"; done
  fn=$(printf '%s\n' "$old" | head -n 1)
  printf '#[test]\nfn %s() {}\n' "${fn##*::}" > "$T/toy-pins-between/crates/x/src/pins.rs"
  printf '#[tokio::test]\nasync fn a_declared_csv_starting_with_a_dwg_signature_is_accepted() {}\n#[test]\nfn an_aes_128_pdf_is_read() {}\n' > "$T/toy-pins-tip/apps/y/src/files.rs"
  got=$(expected_pins "$T/toy-pins-branch")
  if [ "$got" = "$P14_PINS" ] && [ -z "$(expected_pins "$T/toy-pins-tip")" ] \
     && ! expected_pins "$T/toy-pins-between" > /dev/null 2> "$T/logs/selftest-08-between.log" \
     && grep -q 'carries part of P15' "$T/logs/selftest-08-between.log"; then
    verdict viii-pins 0 "a branch point expects all seven P14 pins to fail, a tip carrying P15–P18 none, and a tree between them is refused"
  else
    verdict viii-pins 1 "branch [$(printf '%s\n' "$got" | grep -c .)], tip [$(expected_pins "$T/toy-pins-tip" | grep -c .)]; see logs/selftest-08-between.log"
  fi

  # (ix) the suite's verdict (MF-6) on cargo's own output: a toy workspace whose crate p fails one
  # test — the pin, its panic message a line starting `error:` in the captured output, as a real pin's
  # may — and whose crate q aborts (SIGABRT: no FAILED line, no `test result:` of its own)
  local st="$T/toy-suite" log_a log_b log_c rc_a rc_b pin_line="tests::pinned toy"
  mkdir -p "$st/p/src" "$st/q/src"
  printf '[workspace]\nmembers = ["p", "q"]\nresolver = "2"\n' > "$st/Cargo.toml"
  for t in p q; do printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n' "$t" > "$st/$t/Cargo.toml"; done
  printf '#[cfg(test)]\nmod tests {\n    #[test]\n    fn passes() {}\n    #[test]\n    fn pinned() { panic!("error: the behaviour this pin fixes moved"); }\n}\n' > "$st/p/src/lib.rs"
  printf '#[cfg(test)]\nmod tests {\n    #[test]\n    fn aborts() { std::process::abort(); }\n}\n' > "$st/q/src/lib.rs"
  cd "$st"
  run_logged_rc selftest-09a-suite-abort "${CARGO[@]}" test --offline --workspace --no-fail-fast; rc_a=$LAST_RC
  run_logged_rc selftest-09b-suite-pin "${CARGO[@]}" test --offline --workspace --no-fail-fast --exclude q; rc_b=$LAST_RC
  run_logged_rc selftest-09c-suite-pass "${CARGO[@]}" test --offline -p p -- --skip pinned
  cd "$toy"
  log_a="$T/logs/selftest-09a-suite-abort.log"; log_b="$T/logs/selftest-09b-suite-pin.log"; log_c="$T/logs/selftest-09c-suite-pass.log"
  rc=0; ( suite_verdict "$log_b" "$rc_b" "$pin_line" 1 ) > "$T/logs/selftest-09d-verdict-control.log" 2>&1 || rc=$?
  verdict ix-control "$rc" "only the pin fails (rc=$rc_b, in -p p --lib alone, its output a line starting 'error:'): the verdict passes (rc=$rc)"
  rc=0; ( suite_verdict "$log_a" "$rc_a" "$pin_line" 1; touch "$T/must-not-exist-12" ) > "$T/logs/selftest-09e-verdict-abort.log" 2>&1 || rc=$?
  if [ "$rc_a" -ne 0 ] && [ "$rc" -ne 0 ] && [ ! -e "$T/must-not-exist-12" ] && grep -q 'killed by a signal' "$T/logs/selftest-09e-verdict-abort.log"; then
    verdict ix-sigabrt 0 "the pin fails and another binary aborts (SIGABRT): the verdict stopped the driver (rc=$rc; logs/selftest-09e-verdict-abort.log)"
  else
    verdict ix-sigabrt 1 "cargo rc=$rc_a, verdict rc=$rc; see logs/selftest-09e-verdict-abort.log"
  fi
  grep -v '(signal: ' "$log_a" > "$T/logs/selftest-09f-suite-abort-unsignalled.log"
  rc=0; ( suite_verdict "$T/logs/selftest-09f-suite-abort-unsignalled.log" "$rc_a" "$pin_line" 1; touch "$T/must-not-exist-13" ) > "$T/logs/selftest-09g-verdict-binaries.log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ ! -e "$T/must-not-exist-13" ] && grep -q 'not exactly those of P14' "$T/logs/selftest-09g-verdict-binaries.log" && grep -q -- '-p q --lib' "$T/logs/selftest-09g-verdict-binaries.log"; then
    verdict ix-binaries 0 "the same log without its signal line: cargo names -p q --lib, where no pin failed — the verdict stopped the driver (rc=$rc)"
  else
    verdict ix-binaries 1 "rc=$rc; see logs/selftest-09g-verdict-binaries.log"
  fi
  rc=0; ( suite_verdict "$log_c" 101 "" 0; touch "$T/must-not-exist-14" ) > "$T/logs/selftest-09h-verdict-rc.log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ ! -e "$T/must-not-exist-14" ] && grep -q 'no test is expected to fail on this tree, and cargo test exited rc=101' "$T/logs/selftest-09h-verdict-rc.log"; then
    verdict ix-no-pins 0 "no pin expected and a non-zero exit with nothing failed in the log: the verdict stopped the driver (rc=$rc)"
  else
    verdict ix-no-pins 1 "rc=$rc; see logs/selftest-09h-verdict-rc.log"
  fi
  rc=0; ( suite_verdict "$log_b" "$rc_b" "$pin_line" 2; touch "$T/must-not-exist-15" ) > "$T/logs/selftest-09i-verdict-floor.log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ ! -e "$T/must-not-exist-15" ] && grep -q 'passed 1 tests, below the floor of 2' "$T/logs/selftest-09i-verdict-floor.log"; then
    verdict ix-floor 0 "1 passed against a floor of 2: the verdict stopped the driver (rc=$rc)"
  else
    verdict ix-floor 1 "rc=$rc; see logs/selftest-09i-verdict-floor.log"
  fi

  # (x) the freshness gate (MF-2) with no network: a stub date source and a stand-in for cargo that
  # rewrites the lock as a leaf --precise would. alpha 1.0.2 and beta 1.0.2 are younger than the max
  # date, and beta depends on alpha — against the names' order, so a name sort cannot pass: beta is
  # stepped back first (its 1.0.1 is yanked: to the parent's 1.0.0), then alpha (to 1.0.1)
  local fr="$T/toy-fresh" reg='source = "registry+https://github.com/rust-lang/crates.io-index"'
  mkdir -p "$fr"
  printf 'version = 4\n\n[[package]]\nname = "alpha"\nversion = "1.0.0"\n%s\n\n[[package]]\nname = "beta"\nversion = "1.0.0"\n%s\ndependencies = [\n "alpha",\n]\n\n[[package]]\nname = "member"\nversion = "0.1.0"\ndependencies = [\n "beta",\n]\n' "$reg" "$reg" > "$fr/parent.lock"
  sed 's/^version = "1.0.0"$/version = "1.0.2"/' "$fr/parent.lock" > "$fr/young.lock"
  cat > "$fr/dates.json" <<'EOF'
{"crates": {
  "alpha": [{"num": "1.0.0", "created_at": "2026-09-01T00:00:00Z", "yanked": false},
            {"num": "1.0.1", "created_at": "2026-10-01T00:00:00.123456Z", "yanked": false},
            {"num": "1.0.2", "created_at": "2026-10-14T18:52:10Z", "yanked": false},
            {"num": "2.0.0", "created_at": "2026-09-02T00:00:00Z", "yanked": false}],
  "beta":  [{"num": "1.0.0", "created_at": "2026-09-01T00:00:00Z", "yanked": false},
            {"num": "1.0.1", "created_at": "2026-10-02T00:00:00Z", "yanked": true},
            {"num": "1.0.2", "created_at": "2026-10-15T00:00:00Z", "yanked": false}]},
 "npm": {
  "@tauri-apps/api": {"time": {"2.12.3": "2026-10-10T17:44:26.833Z"}, "optional": {"2.12.3": {}}},
  "@tauri-apps/cli": {"time": {"2.12.2": "2026-10-10T20:02:07.833Z"},
                      "optional": {"2.12.2": {"@tauri-apps/cli-linux-arm64-musl": "2.12.2", "fsevents": "~2.3.3"}}},
  "@tauri-apps/cli-linux-arm64-musl": {"time": {"2.12.2": "2026-10-12T20:07:30.211Z"}, "optional": {"2.12.2": {}}}}}
EOF
  python3 -I -c 'import json, sys
d = json.load(open(sys.argv[1])); del d["crates"]["beta"]; json.dump(d, open(sys.argv[2], "w"))' "$fr/dates.json" "$fr/dates-no-beta.json"   # beta cannot be dated
  cat > "$fr/fake-cargo" <<'EOF'
#!/usr/bin/env bash
# fake-cargo update -p NAME@VERSION --precise TARGET: rewrites ./Cargo.lock as cargo would for a leaf
[ "$1" = update ] && [ "$2" = -p ] && [ "$4" = --precise ] || { echo "fake-cargo: unexpected: $*" >&2; exit 2; }
name=${3%@*}; ver=${3##*@}
if [ "${FAKE_CARGO_REFUSE:-}" = "$name" ]; then
  echo "error: failed to select a version for the requirement \`$name = \"^$ver\"\`" >&2; exit 101
fi
N="$name" V="$ver" TO="$5" perl -0pi -e 's/(name = "\Q$ENV{N}\E"\nversion = ")\Q$ENV{V}\E"/$1$ENV{TO}"/' Cargo.lock
echo "    Updating $name v$ver -> v$5" >&2
EOF
  chmod +x "$fr/fake-cargo"
  printf 'alpha@1.0.2  the pair waits on it (self-test)\nbeta@1.0.2  a security patch (self-test)\n' > "$fr/waivers.txt"
  fresh_env() { # run in a subshell: the freshness gate in $fr, the stub date source, the fake cargo
    cd "$fr" && cp young.lock Cargo.lock
    CARGO=("$fr/fake-cargo"); MAX_DATE="2026-10-10T20:08:00Z"; DATE_SOURCE=${1:-$fr/dates.json}
    STEPBACKS_FILE="$T/logs/selftest-10-step-backs-$2.txt"; : > "$STEPBACKS_FILE"
    regflags_init
  }
  rc=0; ( fresh_env "" stops; APPLY_STEPBACKS=0; freshness selftest-x-stops "$fr/parent.lock"; touch "$T/must-not-exist-16" ) > "$T/logs/selftest-10a-fresh-stops.log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ ! -e "$T/must-not-exist-16" ] && grep -q 'rerun with --apply-step-backs' "$T/logs/selftest-10a-fresh-stops.log" \
     && [ "$(sed -n 's/^step-back: //p' "$T/logs/fresh-selftest-x-stops-0.log" | tr '\n' ';')" = "beta@1.0.2 1.0.0;alpha@1.0.2 1.0.1;" ] \
     && cmp -s "$fr/Cargo.lock" "$fr/young.lock"; then
    verdict x-stops 0 "two young entries without --apply-step-backs: the run stopped, the sequence named dependents first (beta@1.0.2 -> 1.0.0, then alpha@1.0.2 -> 1.0.1), the lock untouched"
  else
    verdict x-stops 1 "rc=$rc; see logs/selftest-10a-fresh-stops.log, logs/fresh-selftest-x-stops-0.log"
  fi
  rc=0; ( fresh_env "" apply; APPLY_STEPBACKS=1; freshness selftest-x-apply "$fr/parent.lock" ) > "$T/logs/selftest-10b-fresh-apply.log" 2>&1 || rc=$?
  if [ "$rc" -eq 0 ] && [ "$(lock_versions alpha "$fr/Cargo.lock")" = 1.0.1 ] && [ "$(lock_versions beta "$fr/Cargo.lock")" = 1.0.0 ] \
     && [ "$(sed -n 's/^step selftest-x-apply: cargo update -p \([^ ]*\) --precise \([^ ]*\) .*/\1>\2/p' "$T/logs/selftest-10-step-backs-apply.txt" | tr '\n' ';')" = "beta@1.0.2>1.0.0;alpha@1.0.2>1.0.1;" ]; then
    verdict x-apply 0 "under --apply-step-backs: beta (the dependent) stepped back first, then alpha, the gate run again after each, then clean; each step-back recorded (logs/selftest-10-step-backs-apply.txt)"
  else
    verdict x-apply 1 "rc=$rc; see logs/selftest-10b-fresh-apply.log"
  fi
  rc=0; ( fresh_env "" refused; APPLY_STEPBACKS=1; export FAKE_CARGO_REFUSE=beta; freshness selftest-x-refused "$fr/parent.lock"; touch "$T/must-not-exist-17" ) > "$T/logs/selftest-10c-fresh-refused.log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ ! -e "$T/must-not-exist-17" ] && grep -q 'cargo refused the step-back beta@1.0.2 -> 1.0.0' "$T/logs/selftest-10c-fresh-refused.log" \
     && [ ! -s "$T/logs/selftest-10-step-backs-refused.txt" ]; then
    verdict x-refused 0 "a step-back cargo refuses stops the driver as a named blocker, nothing recorded as applied (rc=$rc)"
  else
    verdict x-refused 1 "rc=$rc; see logs/selftest-10c-fresh-refused.log"
  fi
  rc=0; ( fresh_env "" waived; APPLY_STEPBACKS=0; WAIVERS="$fr/waivers.txt"; regflags_init; freshness selftest-x-waived "$fr/parent.lock" ) > "$T/logs/selftest-10d-fresh-waived.log" 2>&1 || rc=$?
  if [ "$rc" -eq 0 ] && cmp -s "$fr/Cargo.lock" "$fr/young.lock" && [ "$(grep -c '^  waived ' "$T/logs/fresh-selftest-x-waived-0.log")" = 2 ]; then
    verdict x-waived 0 "the same two versions listed as recorded DU-23 (c) waivers: the gate admits them, nothing stepped back"
  else
    verdict x-waived 1 "rc=$rc; see logs/selftest-10d-fresh-waived.log"
  fi
  rc=0; ( fresh_env "$fr/dates-no-beta.json" nolookup; APPLY_STEPBACKS=1; freshness selftest-x-nolookup "$fr/parent.lock"; touch "$T/must-not-exist-18" ) > "$T/logs/selftest-10e-fresh-nolookup.log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ ! -e "$T/must-not-exist-18" ] && grep -q 'gave no verdict' "$T/logs/selftest-10e-fresh-nolookup.log"; then
    verdict x-nolookup 0 "a version the date source cannot date (a failed lookup) is no verdict: the driver stopped (rc=$rc)"
  else
    verdict x-nolookup 1 "rc=$rc; see logs/selftest-10e-fresh-nolookup.log"
  fi
  # the first entry of the sequence has nothing eligible: beta 1.0.2 (the dependent) against a parent
  # holding beta 0.9.0, its 1.0.0 and 1.0.1 yanked — no-step-back first, then alpha's step-back. The
  # run ends there, before any --precise: alpha (the dependency) is never stepped back first
  perl -0pe 's/(name = "beta"\nversion = ")1\.0\.0"/${1}0.9.0"/' "$fr/parent.lock" > "$fr/parent-beta-0.9.lock"
  python3 -I -c 'import json, sys
d = json.load(open(sys.argv[1])); b = d["crates"]["beta"]
b[0]["yanked"] = True; b.insert(0, {"num": "0.9.0", "created_at": "2026-08-01T00:00:00Z", "yanked": False})
json.dump(d, open(sys.argv[2], "w"))' "$fr/dates.json" "$fr/dates-beta-none.json"
  rc=0; ( fresh_env "$fr/dates-beta-none.json" none-first; APPLY_STEPBACKS=1; freshness selftest-x-none-first "$fr/parent-beta-0.9.lock"; touch "$T/must-not-exist-21" ) > "$T/logs/selftest-10h-fresh-none-first.log" 2>&1 || rc=$?
  if [ "$rc" -ne 0 ] && [ ! -e "$T/must-not-exist-21" ] \
     && grep -q 'beta@1.0.2, the first entry of the --precise sequence (dependents first), is younger than 7 days with no eligible version' "$T/logs/selftest-10h-fresh-none-first.log" \
     && [ "$(grep -E '^(no-)?step-back: ' "$T/logs/fresh-selftest-x-none-first-0.log" | tr '\n' ';')" = "no-step-back: beta@1.0.2;step-back: alpha@1.0.2 1.0.1;" ] \
     && [ ! -e "$T/logs/fresh-selftest-x-none-first-0-precise.log" ] && [ ! -s "$T/logs/selftest-10-step-backs-none-first.txt" ] \
     && cmp -s "$fr/Cargo.lock" "$fr/young.lock"; then
    verdict x-none-first 0 "the dependent first in the sequence with nothing eligible: the run stopped naming it, before any --precise (alpha, its dependency, not stepped back; the lock untouched, nothing recorded)"
  else
    verdict x-none-first 1 "rc=$rc; see logs/selftest-10h-fresh-none-first.log, logs/fresh-selftest-x-none-first-0.log"
  fi
  rc=0
  python3 -I "$HERE/lockdates.py" --pins npm --max-date 2026-10-10T20:08Z --date-source "$fr/dates.json" @tauri-apps/api@2.12.3 > "$T/logs/selftest-10f-npm-old.log" 2>&1 || rc=$?
  rc2=0
  python3 -I "$HERE/lockdates.py" --pins npm --max-date 2026-10-10T20:08Z --date-source "$fr/dates.json" @tauri-apps/api@2.12.3 @tauri-apps/cli@2.12.2 > "$T/logs/selftest-10g-npm-binary.log" 2>&1 || rc2=$?
  if [ "$rc" -eq 0 ] && [ "$rc2" -eq 1 ] && grep -q '^  TOO YOUNG  @tauri-apps/cli-linux-arm64-musl 2.12.2' "$T/logs/selftest-10g-npm-binary.log" \
     && grep -q '^  eligible   @tauri-apps/cli 2.12.2' "$T/logs/selftest-10g-npm-binary.log"; then
    verdict x-npm 0 "the npm pins: an eligible pin passes; a cli whose own time is eligible but one platform binary is not is refused (rc=$rc, $rc2)"
  else
    verdict x-npm 1 "rc=$rc, $rc2; see logs/selftest-10f-npm-old.log, logs/selftest-10g-npm-binary.log"
  fi

  # (xi) --not-before (MF-1 item 3): a run refuses to start before the instant (exit 2, no scratch
  # root made), a past instant passes, and an instant without its zone is refused in any mode
  mkdir -p "$T/xi-tmp" "$T/xi-tmp2"
  rc=0
  TMPDIR="$T/xi-tmp" bash "$HERE/gbase.sh" --base HEAD --steps 1 --repo "$T/not-a-repo" --not-before 2099-01-01T00:00Z \
    > "$T/logs/selftest-11a-not-before-future.log" 2>&1 || rc=$?
  if [ "$rc" -eq 2 ] && grep -q 'refusing to start before the cut instant 2099-01-01T00:00Z' "$T/logs/selftest-11a-not-before-future.log" \
     && [ -z "$(ls -A "$T/xi-tmp")" ]; then
    verdict xi-future 0 "a run before --not-before 2099-01-01T00:00Z refused with exit 2, and made no scratch root"
  else
    verdict xi-future 1 "rc=$rc; see logs/selftest-11a-not-before-future.log"
  fi
  rc=0; ( refuse_before "$(instant_epoch 2000-01-01T00:00:00Z)" ) > "$T/logs/selftest-11b-not-before-past.log" 2>&1 || rc=$?
  verdict xi-past "$rc" "a cut instant in the past lets the run start (rc=$rc)"
  rc=0
  TMPDIR="$T/xi-tmp2" bash "$HERE/gbase.sh" --base HEAD --steps 1 --repo "$T/not-a-repo" --dry-run --not-before 2026-10-17 \
    > "$T/logs/selftest-11c-not-before-date.log" 2>&1 || rc=$?
  rc2=0
  TMPDIR="$T/xi-tmp2" bash "$HERE/gbase.sh" --base HEAD --steps 1 --repo "$T/not-a-repo" --dry-run --not-before 2026-10-17T20:08 \
    > "$T/logs/selftest-11d-not-before-nozone.log" 2>&1 || rc2=$?
  if [ "$rc" -eq 2 ] && [ "$rc2" -eq 2 ] && [ -z "$(ls -A "$T/xi-tmp2")" ] \
     && grep -q 'takes the cut instant with its zone' "$T/logs/selftest-11c-not-before-date.log" \
     && grep -q 'takes the cut instant with its zone' "$T/logs/selftest-11d-not-before-nozone.log"; then
    verdict xi-zone 0 "--not-before 2026-10-17 and 2026-10-17T20:08 (no zone) are refused with exit 2, even with --dry-run"
  else
    verdict xi-zone 1 "rc=$rc, $rc2; see logs/selftest-11c-not-before-date.log, logs/selftest-11d-not-before-nozone.log"
  fi

  # (xii) the Tauri set (MF-1 item 2): every crate held at exactly its expected version and each
  # published at or before the max date; one internal a patch off, or one too young, stops the driver
  local tt="$T/toy-tauri" name
  mkdir -p "$tt"; : > "$tt/exact.lock"
  for name in tauri $TAURI_INTERNALS tauri-plugin-dialog; do
    printf '[[package]]\nname = "%s"\nversion = "%s"\n%s\n\n' "$name" "$(tauri_expected "$name")" "$reg" >> "$tt/exact.lock"
  done
  python3 -I -c 'import json, re, sys
held = re.findall(r"name = \"(.*)\"\nversion = \"(.*)\"", open(sys.argv[1]).read())
d = {"crates": {n: [{"num": v, "created_at": "2026-09-30T20:42:58Z", "yanked": False}] for n, v in held}}
json.dump(d, open(sys.argv[2], "w"))
d["crates"]["tauri-build"][0]["created_at"] = "2026-10-14T18:51:08Z"
json.dump(d, open(sys.argv[3], "w"))' "$tt/exact.lock" "$tt/dates.json" "$tt/dates-young.json"
  N=tauri-utils V="$(tauri_expected tauri-utils)" perl -0pe 's/(name = "\Q$ENV{N}\E"\nversion = ")\Q$ENV{V}\E"/${1}9.9.9"/' "$tt/exact.lock" > "$tt/off.lock"
  tauri_case() { # tauri_case LOCK DATES WHERE — in a subshell: expect_tauri_set in $tt on that lock and date source
    cd "$tt" && cp "$1" Cargo.lock && MAX_DATE="2026-10-10T20:08:00Z" && DATE_SOURCE="$2" && regflags_init && expect_tauri_set "$3"
  }
  rc=0; ( tauri_case exact.lock "$tt/dates.json" "selftest tauri" ) > "$T/logs/selftest-12a-tauri-holds.log" 2>&1 || rc=$?
  verdict xii-holds "$rc" "the ten Tauri crates at their expected versions, each published before the max date: the set holds (rc=$rc)"
  rc=0; ( tauri_case off.lock "$tt/dates.json" "selftest tauri-off"; touch "$T/must-not-exist-19" ) > "$T/logs/selftest-12b-tauri-off.log" 2>&1 || rc=$?
  rc2=0; ( tauri_case exact.lock "$tt/dates-young.json" "selftest tauri-young"; touch "$T/must-not-exist-20" ) > "$T/logs/selftest-12c-tauri-young.log" 2>&1 || rc2=$?
  if [ "$rc" -ne 0 ] && [ "$rc2" -ne 0 ] && [ ! -e "$T/must-not-exist-19" ] && [ ! -e "$T/must-not-exist-20" ] \
     && grep -q "the lock holds tauri-utils at '9.9.9 '" "$T/logs/selftest-12b-tauri-off.log" \
     && grep -q 'younger than 7 days at the cut' "$T/logs/selftest-12c-tauri-young.log"; then
    verdict xii-broken 0 "tauri-utils one version off, and tauri-build published after the max date, each stopped the driver (rc=$rc, $rc2)"
  else
    verdict xii-broken 1 "rc=$rc, $rc2; see logs/selftest-12b-tauri-off.log, logs/selftest-12c-tauri-young.log"
  fi

  # (xiii) MF-5: a `[build] build-dir` in the tree's .cargo/config.toml (M63's form) keeps nothing of a
  # driver build outside the target dir; the control — the same build without CARGO_BUILD_BUILD_DIR —
  # shows that the config does move the intermediates, so the case is not vacuous
  mkdir -p "$toy/.cargo"
  printf '[build]\nbuild-dir = "%s"\n' "$T/leak-build-dir" > "$toy/.cargo/config.toml"
  cargo_cmd "$T/target-bd"
  run_logged_rc selftest-13a-build-dir-driver "${CARGO_CMD[@]}" check --offline; rc=$LAST_RC
  local leak_driver=no leak_control=no
  [ ! -e "$T/leak-build-dir" ] || leak_driver=yes
  run_logged_rc selftest-13b-build-dir-control nice -n 10 env -u CARGO_BUILD_BUILD_DIR CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0 "CARGO_TARGET_DIR=$T/target-bd-control" cargo "+$TOOLCHAIN" check --offline; rc2=$LAST_RC
  [ ! -d "$T/leak-build-dir" ] || [ -z "$(find "$T/leak-build-dir" -type f | head -n 1)" ] || leak_control=yes
  rm -rf "$toy/.cargo"
  if [ "$rc" -eq 0 ] && [ "$rc2" -eq 0 ] && [ "$leak_driver" = no ] && [ "$leak_control" = yes ] && [ -n "$(find "$T/target-bd" -type f | head -n 1)" ]; then
    verdict xiii-build-dir 0 "with a configured build-dir, the driver's cargo wrote nothing there and everything under its target dir; without CARGO_BUILD_BUILD_DIR the same check wrote there"
  else
    verdict xiii-build-dir 1 "rc=$rc, $rc2; driver leaked: $leak_driver, control leaked: $leak_control"
  fi

  # (xiv) A1-17: the evidence archive holds logs/ and locks/ only, and --archive-to gets a copy
  mkdir -p "$T/archive-out"
  rc=0; ( ARCHIVE=1; ARCHIVE_TO="$T/archive-out"; archive_evidence ) > "$T/logs/selftest-14-archive.log" 2>&1 || rc=$?
  local listing
  listing=$(tar -tzf "$T/evidence-$RUN_DATE-gbase.tar.gz" 2>/dev/null | sed 's|^\./||' | awk -F/ '{ print $1 }' | sort -u | tr '\n' ' ')
  if [ "$rc" -eq 0 ] && [ "$listing" = "locks logs " ] && cmp -s "$T/evidence-$RUN_DATE-gbase.tar.gz" "$T/archive-out/evidence-$RUN_DATE-gbase.tar.gz"; then
    verdict xiv-archive 0 "the evidence archive holds logs/ and locks/ only, and --archive-to received the same file"
  else
    verdict xiv-archive 1 "rc=$rc, top-level entries: $listing; see logs/selftest-14-archive.log"
  fi

  # (xv) C5: the bun.lock check (lockdates.py --bun-lock) on toy locks written as Bun writes them —
  # trailing commas, a workspace entry, an aliased key holding another name, a pair held twice — and a
  # stub date source with no network: @scope/right 2.0.1 published exactly at the max date (eligible),
  # young 3.1.0 one second after it (too young), reached only through a nested (transitive) key
  local bl="$T/toy-bunlock" bmax="2026-10-14T19:53:00Z"
  mkdir -p "$bl"
  cat > "$bl/ok.lock" <<'EOF'
{
  "lockfileVersion": 2,
  "configVersion": 1,
  "workspaces": {
    "": {
      "name": "toy",
    },
    "apps/gui": {
      "name": "toy-gui",
      "dependencies": {
        "@scope/right": "2.0.1",
        "left": "1.0.0",
      },
    },
  },
  "packages": {
    "@scope/right": ["@scope/right@2.0.1", "", { "dependencies": { "left": "^1.0.0" } }, "sha512-r"],

    "left": ["left@1.0.0", "", {}, "sha512-l"],

    "right-alias": ["@scope/right@2.0.1", "", { "dependencies": { "left": "^1.0.0" } }, "sha512-r"],

    "toy-gui": ["toy-gui@workspace:apps/gui"],
  }
}
EOF
  perl -0pe 's/(\n    "toy-gui": )/\n    "\@scope\/right\/young": ["young\@3.1.0", "", {}, "sha512-y"],\n$1/' "$bl/ok.lock" > "$bl/young.lock"
  perl -0pe 's/(\n    "toy-gui": )/\n    "gitdep": ["gitdep\@github:owner\/gitdep#0123abc", {}, "owner-gitdep-0123abc"],\n$1/' "$bl/ok.lock" > "$bl/git.lock"
  perl -0pe 's/(\n    "toy-gui": )/\n    "private": ["private\@1.0.0", "https:\/\/npm.example.test\/", {}, "sha512-p"],\n$1/' "$bl/ok.lock" > "$bl/other-registry.lock"
  printf '{\n  "lockfileVersion": 2,\n  "workspaces": {},\n}\n' > "$bl/no-packages.lock"
  cat > "$bl/dates.json" <<'EOF'
{"crates": {}, "npm": {
  "left": {"time": {"1.0.0": "2026-09-01T00:00:00.000Z"}, "optional": {"1.0.0": {}}},
  "@scope/right": {"time": {"2.0.1": "2026-10-14T19:53:00.000Z"}, "optional": {"2.0.1": {}}},
  "young": {"time": {"3.1.0": "2026-10-14T19:53:01.000Z"}, "optional": {"3.1.0": {}}}}}
EOF
  python3 -I -c 'import json, sys
d = json.load(open(sys.argv[1])); del d["npm"]["left"]; json.dump(d, open(sys.argv[2], "w"))' "$bl/dates.json" "$bl/dates-no-left.json"   # left cannot be dated
  bunlock_run() { # bunlock_run LOCK DATES LOG — lockdates.py --bun-lock on a toy lock, rc in BL_RC
    BL_RC=0; python3 -I "$HERE/lockdates.py" --bun-lock "$1" --max-date "$bmax" --date-source "$2" > "$T/logs/$3.log" 2>&1 || BL_RC=$?
  }
  bunlock_run "$bl/ok.lock" "$bl/dates.json" selftest-15a-bunlock-holds
  if [ "$BL_RC" -eq 0 ] \
     && grep -qF 'entries: 4; from the npm registry: 3 (2 distinct name@version); not from a registry, not dated: 1 (toy-gui@workspace:apps/gui)' "$T/logs/selftest-15a-bunlock-holds.log" \
     && grep -qF 'checked: 2  too young: 0' "$T/logs/selftest-15a-bunlock-holds.log" \
     && grep -q '^  eligible   @scope/right 2.0.1  2026-10-14T19:53:00.000Z$' "$T/logs/selftest-15a-bunlock-holds.log"; then
    verdict xv-bunlock-holds 0 "a bun.lock with trailing commas: every entry read by its value (the aliased key's @scope/right counted once), the workspace entry not dated, a release published exactly at the max date eligible (rc=0)"
  else
    verdict xv-bunlock-holds 1 "rc=$BL_RC; see logs/selftest-15a-bunlock-holds.log"
  fi
  bunlock_run "$bl/young.lock" "$bl/dates.json" selftest-15b-bunlock-young
  if [ "$BL_RC" -eq 1 ] && grep -q '^  TOO YOUNG  young 3.1.0  2026-10-14T19:53:01.000Z$' "$T/logs/selftest-15b-bunlock-young.log" \
     && grep -qF 'checked: 3  too young: 1' "$T/logs/selftest-15b-bunlock-young.log"; then
    verdict xv-bunlock-young 0 "a transitive entry published one second after the max date, reached only through a nested key, is refused (rc=1)"
  else
    verdict xv-bunlock-young 1 "rc=$BL_RC; see logs/selftest-15b-bunlock-young.log"
  fi
  bunlock_run "$bl/ok.lock" "$bl/dates-no-left.json" selftest-15c-bunlock-nolookup
  rc=$BL_RC
  bunlock_run "$bl/git.lock" "$bl/dates.json" selftest-15d-bunlock-source
  rc2=$BL_RC
  bunlock_run "$bl/other-registry.lock" "$bl/dates.json" selftest-15d-bunlock-other-registry
  if [ "$rc" -eq 2 ] && [ "$rc2" -eq 2 ] && [ "$BL_RC" -eq 2 ] && grep -q 'left@1.0.0 is not in the date source' "$T/logs/selftest-15c-bunlock-nolookup.log" \
     && grep -q 'gitdep@github:owner/gitdep#0123abc .* is not from the npm registry' "$T/logs/selftest-15d-bunlock-source.log" \
     && grep -q "private@1.0.0 .* comes from the registry 'https://npm.example.test/'" "$T/logs/selftest-15d-bunlock-other-registry.log"; then
    verdict xv-bunlock-nolookup 0 "an entry the date source cannot date, a git entry, and an entry from another registry are each no verdict (rc=$rc, $rc2, $BL_RC)"
  else
    verdict xv-bunlock-nolookup 1 "rc=$rc, $rc2, $BL_RC; see logs/selftest-15c-bunlock-nolookup.log, logs/selftest-15d-bunlock-source.log, logs/selftest-15d-bunlock-other-registry.log"
  fi
  bunlock_run "$T/locks/selftest-lock-A.lock" "$bl/dates.json" selftest-15e-bunlock-cargo-lock
  rc=$BL_RC
  bunlock_run "$bl/no-packages.lock" "$bl/dates.json" selftest-15f-bunlock-no-packages
  if [ "$rc" -eq 2 ] && [ "$BL_RC" -eq 2 ] && grep -q 'is not a bun.lock (not JSON' "$T/logs/selftest-15e-bunlock-cargo-lock.log" \
     && grep -q 'is not a bun.lock: no `packages` block' "$T/logs/selftest-15f-bunlock-no-packages.log"; then
    verdict xv-bunlock-notalock 0 "a Cargo.lock, and JSON with no packages block, are refused as no verdict (rc=$rc, $BL_RC)"
  else
    verdict xv-bunlock-notalock 1 "rc=$rc, $BL_RC; see logs/selftest-15e-bunlock-cargo-lock.log, logs/selftest-15f-bunlock-no-packages.log"
  fi
  bunlock_case() { # bunlock_case LOCK LOG — in a subshell: the driver's step, bun_lock_dates, in $bl on that lock
    cd "$bl" && cp "$1" bun.lock && MAX_DATE="$bmax" && DATE_SOURCE="$bl/dates.json" && WAIVERS="" && regflags_init && bun_lock_dates "$2"
  }
  rc=0; ( bunlock_case "$bl/ok.lock" selftest-15g-step-holds ) > "$T/logs/selftest-15g-driver-holds.log" 2>&1 || rc=$?
  rc2=0; ( bunlock_case "$bl/young.lock" selftest-15h-step-young; touch "$T/must-not-exist-22" ) > "$T/logs/selftest-15h-driver-young.log" 2>&1 || rc2=$?
  if [ "$rc" -eq 0 ] && [ "$rc2" -ne 0 ] && [ ! -e "$T/must-not-exist-22" ] \
     && grep -q 'bun.lock freshness (2026-10-14T19:53:00Z): lockfileVersion 2; entries: 4' "$T/logs/selftest-15g-driver-holds.log" \
     && grep -q 'the new bun.lock holds 1 version(s) published after 2026-10-14T19:53:00Z' "$T/logs/selftest-15h-driver-young.log" \
     && grep -q '^  TOO YOUNG  young 3.1.0' "$T/logs/selftest-15h-driver-young.log"; then
    verdict xv-driver 0 "the driver's step after bun install passes the eligible lock and stops the driver on the young one, naming it (rc=$rc, $rc2)"
  else
    verdict xv-driver 1 "rc=$rc, $rc2; see logs/selftest-15g-driver-holds.log, logs/selftest-15h-driver-young.log"
  fi
  # one mode a run: two of --gate, --pins and --bun-lock, or one given twice, are no verdict before anything
  # is read — never the last one alone (the reading this replaces ran the --pins check here and passed it,
  # left 1.0.0 being eligible; the gate on two equal locks passed too; the second --bun-lock, the young one,
  # was checked in place of the first). The refusal's message must stay without the docstring: the docstring
  # says `lockfileVersion`, and the negative grep below would read it as a lock read.
  twomodes_run() { # twomodes_run LOG ARGS... — lockdates.py with the given modes, rc in BL_RC
    local log=$1; shift
    BL_RC=0; python3 -I "$HERE/lockdates.py" "$@" --max-date "$bmax" --date-source "$bl/dates.json" > "$T/logs/$log.log" 2>&1 || BL_RC=$?
  }
  twomodes_run selftest-15i-two-modes-pins --bun-lock "$bl/ok.lock" --pins npm left@1.0.0
  rc=$BL_RC
  twomodes_run selftest-15j-two-modes-gate --bun-lock "$bl/ok.lock" --gate "$T/locks/selftest-lock-A.lock" "$T/locks/selftest-lock-A.lock"
  rc2=$BL_RC
  twomodes_run selftest-15k-two-modes-twice --bun-lock "$bl/ok.lock" --bun-lock "$bl/young.lock"
  if [ "$rc" -eq 2 ] && [ "$rc2" -eq 2 ] && [ "$BL_RC" -eq 2 ] \
     && grep -qF 'no verdict: give one of --gate, --pins or --bun-lock (given: --bun-lock, --pins)' "$T/logs/selftest-15i-two-modes-pins.log" \
     && grep -qF 'no verdict: give one of --gate, --pins or --bun-lock (given: --bun-lock, --gate)' "$T/logs/selftest-15j-two-modes-gate.log" \
     && grep -qF 'no verdict: give one of --gate, --pins or --bun-lock (given: --bun-lock, --bun-lock)' "$T/logs/selftest-15k-two-modes-twice.log" \
     && ! grep -qE 'lockfileVersion|checked:|freshness gate:' "$T/logs/selftest-15i-two-modes-pins.log" "$T/logs/selftest-15j-two-modes-gate.log" "$T/logs/selftest-15k-two-modes-twice.log"; then
    verdict xv-two-modes 0 "--bun-lock with --pins, with --gate, or given twice is no verdict before anything is read, never the last mode alone (rc=$rc, $rc2, $BL_RC)"
  else
    verdict xv-two-modes 1 "rc=$rc, $rc2, $BL_RC; see logs/selftest-15i-two-modes-pins.log, logs/selftest-15j-two-modes-gate.log, logs/selftest-15k-two-modes-twice.log"
  fi

  say ""
  say "SELF-TEST: $pass passed, $fail failed; logs: $T/logs"
  [ "$fail" -eq 0 ] || exit 1
  exit 0
}

# ---------------------------------------------------------------------------- main
main() {
  check_args       # every usage error, the Tauri set, the cut instant — before anything is made
  make_root
  if [ "$SELFTEST" -eq 1 ]; then self_test; fi

  [ -n "$BASE" ] || { echo "gbase: --base <commit> is required (or --self-test)" >&2; usage >&2; exit 2; }
  if [ -z "$REPO" ]; then
    REPO=$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null) || { echo "gbase: $HERE is not inside a repository; pass --repo" >&2; exit 2; }
  fi
  if [ "$REAL" -eq 1 ]; then ARCHIVE=1; trap archive_evidence EXIT; fi
  say "repository: $REPO  base: $BASE  toolchain: $TOOLCHAIN  steps: $STEPS  strict: $STRICT  dry-run: $DRY  edits-only: $EDITS_ONLY"
  say "cut (--not-before): $NOT_BEFORE  freshness max date: $MAX_DATE  step-backs: $( [ "$APPLY_STEPBACKS" -eq 1 ] && echo applied || echo 'a young entry stops the run')  waivers: ${WAIVERS:-none}"
  say "the pins: tauri $TAURI_PRECISE, tauri-plugin-dialog =$DIALOG, @tauri-apps/api $JS_API, @tauri-apps/cli $JS_CLI; the Tauri set: $(printf '%s\n' "$TAURI_SET" | awk '{ printf "%s%s %s", s, $1, $2; s = ", " }')"
  cargo_cmd "$T/target"; CARGO=("${CARGO_CMD[@]}")
  cargo_cmd "$T/target-gui"; CARGO_GUI=("${CARGO_CMD[@]}")
  regflags_init

  preflight

  local last=8
  [ "$STEPS" = all ] || last=$STEPS

  if [ "$DRY" -eq 1 ]; then
    MODE=plan
    plan ""; plan "PLAN (dry run; nothing below was executed) — export of $BASE_SHA, toolchain $TOOLCHAIN, cut $NOT_BEFORE (freshness max date $MAX_DATE)"
    run_steps "$last"
    if [ "$STEPS" = all ]; then
      final_tests
      plan "== final: python3 -I scripts/gen_api_docs.py --check   (expected: rc 1, exactly docs/api/crates/openalpaca_mcp.md)   > logs/91-gen-api-docs.log"
      plan "== final: the JS set and the four bun gates"
      final_js
      final_gui
    fi
    plan ""; plan "DRY RUN OK: steps 1..$last planned$( [ "$STEPS" = all ] && printf ' with the final phase'); pre-flight passed ($ANCHORS_CHECKED anchors, 3 patches, cargo +$TOOLCHAIN). Logs: $T/logs"
    rm -rf "$T/src"
    exit 0
  fi

  MODE=apply
  run_steps "$last"
  if [ "$EDITS_ONLY" -eq 1 ]; then
    [ "$STEPS" = all ] && js_edits
    say ""; say "EDITS-ONLY OK: the manifest edits and patches of steps up to $last$( [ "$STEPS" = all ] && printf ' and the JS edits') are applied in $T/src; no cargo ran."
    exit 0
  fi
  if [ "$STEPS" != all ]; then
    summary
    say "STOPPED after step $last as requested (--steps $last); the export is $T/src"
    exit 0
  fi
  final_tests
  final_docs_check
  final_js
  final_gui
  summary
  if [ "$STRICT" -eq 1 ] && [ "$LOCK_UNEXPECTED" -gt 0 ]; then die "--strict: $LOCK_UNEXPECTED unexpected lock difference(s)"; fi
  exit 0
}

main "$@"
