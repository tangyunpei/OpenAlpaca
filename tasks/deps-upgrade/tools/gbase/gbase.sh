#!/usr/bin/env bash
# gbase.sh — the G-base driver: the dependency wave's re-cut, re-run at the branch point.
#
# plan-rev3.md §5.1 items 6 and 6a (rev 3.1), §5.2's named gate G-base. From one
# canonical scratch root it exports the base commit and runs the EIGHT steps of the
# re-cut in order — each step its anchored manifest edits, its source hunks, its cargo
# commands, a `cargo check` and a lock snapshot compared with the shipped one — then the
# full suite, the docs check, the four bun gates on the P31+P32+P33 JS set and G-GUI.
# It exits non-zero at the FIRST failed command; nothing is recorded-and-continued.
# Every log lands under $T/logs/. README.md beside this file says how to read a run.
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
DATES=0
REPO=""

usage() {
  cat <<'EOF'
usage: gbase.sh --base <commit> [--toolchain <ver>] [--bun <path>] [--steps <N|all>]
                [--strict] [--with-stepbacks] [--dates] [--repo <path>]
       gbase.sh --base <commit> --dry-run       print the plan after the pre-flight checks; exit 0
       gbase.sh --base <commit> --edits-only    export, apply the eight steps' edits and patches, run no cargo
       gbase.sh --self-test [--toolchain <ver>] prove the three failure modes on a toy workspace

  --base <commit>      the branch point to export (`git archive`); required unless --self-test
  --toolchain <ver>    the Rust toolchain the wave pins and every cargo call uses (default 1.99.0;
                       `cargo +<ver>` must already be installed — this script installs nothing)
  --bun <path>         the Bun binary for the four GUI gates (required when the final phase runs)
  --steps <N|all>      run steps 1..N and stop before the final phase (default: all)
  --strict             a lock difference that is not the P04 hold's (steps 3-7) or a requested
                       step-back's is a failure, not a line for a human
  --with-stepbacks     DU-23 (b): after the Tauri hold, step the six packages younger than 7 days
                       back to their eligible versions (off by default: DU-23 (a), a cut on or after
                       2026-10-17 takes the measured set)
  --dates              after the run, look up on crates.io (User-Agent openalpaca-deps-check) when
                       each version a lock difference introduced was published; off by default
  --repo <path>        the repository to export from (default: the one holding this script)

Exit status: 0 — every step and gate passed (lock differences, if any, were printed, and under
--strict there were none); 1 — the first failed command, anchor, patch, comparison or gate (the
message names it and the log); 2 — a usage error or a missing prerequisite.
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
    --dates) DATES=1; shift ;;
    --repo) REPO=${2:?--repo needs a path}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "gbase: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

case "$STEPS" in
  all|1|2|3|4|5|6|7|8) ;;
  *) echo "gbase: --steps takes 1..8 or all, not '$STEPS'" >&2; exit 2 ;;
esac

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

die() { echo "gbase: FAIL: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || { echo "gbase: missing prerequisite: $1" >&2; exit 2; }; }

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

canon() {
  if command -v realpath >/dev/null 2>&1; then realpath "$1"
  else python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "$1"
  fi
}

make_root() {
  local d
  d=$(mktemp -d "${TMPDIR:-/tmp}/gbase.XXXXXX")
  T=$(canon "$d")
  mkdir -p "$T/logs" "$T/locks"
  say "scratch root: $T  ($(now))"
}

# live: the primitives do real work only in apply mode with cargo allowed.
live() { [ "$MODE" = apply ] && [ "$EDITS_ONLY" -eq 0 ]; }

# run_logged_rc NAME CMD... — run CMD, output to $T/logs/NAME.log, LAST_RC set, never exits.
run_logged_rc() {
  local name=$1; shift
  LAST_RC=0
  if [ "$MODE" = plan ]; then plan "    \$ $*   > logs/$name.log"; return 0; fi
  if [ "$MODE" = check ]; then return 0; fi
  if [ "$EDITS_ONLY" -eq 1 ]; then plan "    (edits-only: not run) $*"; return 0; fi
  local log="$T/logs/$name.log" t0 rc=0
  t0=$(date +%s)
  say "    \$ $*   > logs/$name.log"
  "$@" > "$log" 2>&1 || rc=$?
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

# spec_for NAME PATTERN — the `-p` spelling: bare when the lock holds NAME once, NAME@VER otherwise
# (the one version matching PATTERN). P04 (ii): `toml@1.1.8`, never the ambiguous bare `toml`.
spec_for() {
  local name=$1 pat=$2 vers n v
  vers=$(lock_versions "$name")
  n=$(printf '%s\n' "$vers" | grep -c . || true)
  if [ "$n" -le 1 ]; then printf '%s\n' "$name"; return 0; fi
  v=$(printf '%s\n' "$vers" | grep -E -- "$pat" || true)
  [ "$(printf '%s\n' "$v" | grep -c . || true)" = 1 ] \
    || die "cannot spell the step-back for $name: the lock holds $(printf '%s ' $vers)"
  printf '%s@%s\n' "$name" "$v"
}

# compare_lock LABEL EXPECTED ACTUAL [--expected LABEL=NAMES]... — print the differences;
# an unexpected one fails the run under --strict.
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
  [ "$rc" -le 1 ] || { cat "$out" >&2; die "lockdiff.py failed (rc=$rc)"; }
  head=$(head -n 1 "$out")
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

# snapshot_compare N SHIPPED [--expected LABEL=NAMES]... — copy the lock to locks/step-N.lock and compare.
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

# ---------------------------------------------------------------------------- the eight steps
# Every version below is the measured re-cut's (plan-rev3 §5.3, DU-23 (a)); the shipped locks
# under locks/ describe exactly this sequence at 7295f91b.

STEPBACK_LABEL="step-back"
stepback_expect() { # the --expected spec for the six step-backs, or nothing
  if [ "$WITH_STEPBACKS" -eq 1 ]; then printf -- '--expected\n%s=toml,toml_edit,uuid,serde_json,tokio-util,signal-hook\n' "$STEPBACK_LABEL"; fi
}

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

step_2() {
  begin_step 2 "semver refresh, the ten-crate Tauri hold$( [ "$WITH_STEPBACKS" -eq 1 ] && printf ', the DU-23 (b) step-backs')  (P04)"
  run_logged 02-update "${CARGO[@]}" update
  local i=0 pair name ver
  for pair in \
      "tauri-plugin-dialog 2.7.3" "tauri-plugin-fs 2.5.2" "tauri 2.11.6" "tauri-build 2.6.3" \
      "tauri-runtime-wry 2.11.4" "tauri-runtime 2.11.3" "tauri-macros 2.6.3" "tauri-codegen 2.6.3" \
      "tauri-plugin 2.6.3" "tauri-utils 2.9.3"; do
    i=$((i + 1)); name=${pair% *}; ver=${pair#* }
    run_logged "02-hold-$(printf '%02d' "$i")-$name" "${CARGO[@]}" update -p "$name" --precise "$ver"
  done
  if [ "$WITH_STEPBACKS" -eq 1 ]; then
    local triple pat spec
    i=0
    for triple in "toml ^1[.] 1.1.6" "toml_edit ^0[.]25[.] 0.25.15" "uuid . 1.27.0" \
                  "serde_json . 1.0.151" "tokio-util . 0.7.19" "signal-hook . 0.4.4"; do
      i=$((i + 1))
      set -- $triple; name=$1; pat=$2; ver=$3
      if live; then spec=$(spec_for "$name" "$pat"); else spec="$name[@<locked>]"; fi
      run_logged "02-stepback-$i-$name" "${CARGO[@]}" update -p "$spec" --precise "$ver"
    done
  fi
  if live; then
    local p set=""
    for p in tauri tauri-build tauri-plugin-dialog tauri-plugin-fs tauri-utils tauri-runtime tauri-runtime-wry tauri-codegen tauri-macros tauri-plugin wry tao; do
      set="$set$p $(lock_versions "$p" | tr '\n' '/' | sed 's|/$||')  "
    done
    say "    Tauri sub-graph after the hold (P04's body records it): $set"
  fi
  run_logged 02-check "${CARGO[@]}" check --workspace --all-targets --exclude openalpaca_gui
  live && say "    $(count_diag "$T/logs/02-check.log")"
  snapshot_compare 2 recut-lock-02-update-held.lock $(stepback_expect)
}

HOLD_EXPECT="hold=$HERE/hold-footprint.txt"   # the P04 hold's 58 names; steps 3-7 compare with un-held snapshots

step_3() {
  begin_step 3 "storage: rusqlite 0.40.2, lru 0.18, sysinfo 0.39.6  (P06-P08)"
  edit Cargo.toml 'rusqlite = { version = "0.38.0", features = ["bundled"] }' 'rusqlite = { version = "0.40.2", features = ["bundled"] }'
  edit Cargo.toml 'lru = "0.12"' 'lru = "0.18"'
  edit Cargo.toml 'sysinfo = "0.38.1"' 'sysinfo = "0.39.6"'
  run_logged 03-check "${CARGO[@]}" check --workspace --all-targets --exclude openalpaca_gui
  live && say "    $(count_diag "$T/logs/03-check.log")"
  snapshot_compare 3 recut-lock-03-storage.lock --expected "$HOLD_EXPECT" $(stepback_expect)
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
  snapshot_compare 4 recut-lock-04-secrets.lock --expected "$HOLD_EXPECT" $(stepback_expect)
}

step_5() {
  begin_step 5 "documents and sniffing: infer 0.22, pdf-extract 0.12, zip 8.6 (deflate only), calamine 0.36  (P15, P16, P18)"
  edit Cargo.toml 'infer = "0.16"' 'infer = "0.22"'
  edit Cargo.toml 'pdf-extract = "0.8"' 'pdf-extract = "0.12"'
  edit Cargo.toml 'zip = "2"' 'zip = { version = "8.6", default-features = false, features = ["deflate"] }'
  edit Cargo.toml 'calamine = "0.26"' 'calamine = "0.36"'
  run_logged 05-check "${CARGO[@]}" check --workspace --all-targets --exclude openalpaca_gui
  live && say "    $(count_diag "$T/logs/05-check.log")"
  snapshot_compare 5 recut-lock-05-documents.lock --expected "$HOLD_EXPECT" $(stepback_expect)
}

step_6() {
  begin_step 6 "MCP client: rmcp 0.16 -> 3.5.0, the source hunks, the lock pinned at 3.5.0  (P19)"
  edit crates/openalpaca_mcp/Cargo.toml \
    'rmcp = { version = "0.16", features = ["client", "transport-child-process", "transport-streamable-http-client-reqwest"] }' \
    'rmcp = { version = "3.5.0", features = ["client", "transport-child-process", "transport-streamable-http-client-reqwest"] }'
  apply_patch step-6-rmcp
  # Measured: the first --precise resolves the changed manifest and lands on 3.5.1; the second downgrades.
  run_logged 06-rmcp-precise-1 "${CARGO[@]}" update -p rmcp --precise 3.5.0
  live && say "    rmcp after the first --precise: $(lock_versions rmcp | tr '\n' ' ')"
  run_logged 06-rmcp-precise-2 "${CARGO[@]}" update -p rmcp --precise 3.5.0
  run_logged 06-rmcp-macros-precise "${CARGO[@]}" update -p rmcp-macros --precise 3.5.0
  if live; then
    local r m
    r=$(lock_versions rmcp | tr '\n' ' '); m=$(lock_versions rmcp-macros | tr '\n' ' ')
    say "    rmcp: ${r}rmcp-macros: ${m}"
    [ "$r" = "3.5.0 " ] && [ "$m" = "3.5.0 " ] || die "the lock does not pin rmcp 3.5.0 / rmcp-macros 3.5.0 (rmcp: ${r}rmcp-macros: ${m})"
  fi
  run_logged 06-check "${CARGO[@]}" check --workspace --all-targets --exclude openalpaca_gui
  live && say "    $(count_diag "$T/logs/06-check.log")"
  snapshot_compare 6 recut-lock-06-rmcp.lock --expected "$HOLD_EXPECT" $(stepback_expect)
}

step_7() {
  begin_step 7 "web and CLI: tower-http 0.7.1, tokio-tungstenite 0.30.0, rustyline 18  (P25-P27)"
  edit Cargo.toml 'tower-http = { version = "0.6.8", features = ["cors"] }' 'tower-http = { version = "0.7.1", features = ["cors"] }'
  edit Cargo.toml 'tokio-tungstenite = "0.28.0"' 'tokio-tungstenite = "0.30.0"'
  edit Cargo.toml 'rustyline = "17"' 'rustyline = "18"'
  run_logged 07-check "${CARGO[@]}" check --workspace --all-targets --exclude openalpaca_gui
  live && say "    $(count_diag "$T/logs/07-check.log")"
  snapshot_compare 7 recut-lock-07-web.lock --expected "$HOLD_EXPECT" $(stepback_expect)
}

step_8() {
  begin_step 8 "configuration formats: toml 1.1, serde_yaml -> serde-saphyr 1.3  (P29, P30)"
  edit Cargo.toml 'toml = "0.9"' 'toml = "1.1"'
  edit Cargo.toml 'serde_yaml = "0.9"' $'# SKILL.md frontmatter. Replaces the archived serde_yaml 0.9: maintained, pure\n# Rust, and set as close to serde_yaml\'s reading as its options go\n# (`yaml_options` in openalpaca_core::middleware::skill).\nserde-saphyr = "1.3"'
  edit crates/openalpaca_core/Cargo.toml 'serde_yaml.workspace = true' 'serde-saphyr.workspace = true'
  apply_patch step-8-config
  run_logged 08-check "${CARGO[@]}" check --workspace --all-targets --exclude openalpaca_gui
  live && say "    $(count_diag "$T/logs/08-check.log")"
  # The step-8 snapshot IS the wave's final lock: compared with the held final snapshot, no hold
  # filter (recut-lock-08-config.lock, shipped too, is the un-held re-cut's and equals its 99-final).
  snapshot_compare 8 recut-lock-99-final-held.lock $(stepback_expect)
}

run_steps() {
  local last=$1 n
  for n in 1 2 3 4 5 6 7 8; do
    [ "$n" -le "$last" ] || break
    "step_$n"
  done
}

# ---------------------------------------------------------------------------- the final phase
# lexical sort, not -n: `comm` below needs the two lists in `sort`'s own order
sleep_orphans() { ps -o pid= -o ppid= -o command= -A 2>/dev/null | awk '$2 == 1 && /sleep 3600/ && !/awk/ { print $1 }' | sort || true; }

final_tests() {
  CURRENT_STEP="final-tests"
  say ""; say "== final: the full suite   ($(now))"
  local store="$T/test-store" home="$T/test-home" rustup_home="" cargo_home="" before="" after="" new="" p=0 f=0 g=0
  rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"; cargo_home="${CARGO_HOME:-$HOME/.cargo}"
  if live; then mkdir -p "$store/config" "$home"; before=$(sleep_orphans); fi
  run_logged_rc 90-test nice -n 10 env CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=8 \
    "CARGO_TARGET_DIR=$T/target" "OPENALPACA_HOME_STORE=$store" "OPENALPACA_CONFIG_DIR=$store/config" \
    "HOME=$home" "RUSTUP_HOME=$rustup_home" "CARGO_HOME=$cargo_home" \
    cargo "+$TOOLCHAIN" test --workspace --exclude openalpaca_gui --no-fail-fast
  live || return 0
  set -- $(awk '/^test result: /{ for (i = 1; i <= NF; i++) { if ($(i+1) == "passed;") p += $i; if ($(i+1) == "failed;") f += $i; if ($(i+1) == "ignored;") g += $i } } END { print p + 0, f + 0, g + 0 }' "$T/logs/90-test.log")
  p=$1; f=$2; g=$3
  TEST_LINE="$p passed / $f failed / $g ignored (rc=$LAST_RC)"
  say "    cargo test: $TEST_LINE"
  grep -E '^test .* \.\.\. FAILED$|^error: test failed' "$T/logs/90-test.log" | sed 's/^/      /' || true
  after=$(sleep_orphans)
  new=$(comm -13 <(printf '%s\n' $before) <(printf '%s\n' $after) | grep . || true)
  if [ -n "$new" ]; then
    say "    orphaned 'sleep 3600' children left by the daemon's tests (ppid 1, started in this run), killed: $(printf '%s ' $new)"
    kill $new 2>/dev/null || true
  fi
  [ "$LAST_RC" -eq 0 ] && [ "$f" -eq 0 ] || die "the suite failed: $TEST_LINE (log: $T/logs/90-test.log)"
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

# The JS set: P31 (minor/patch pins, measured set under DU-23 (a)) + P32 (Vite 8, Vitest 5)
# + P33 (@vitejs/plugin-react). NOT P31a's @tauri-apps/* pins: the Rust side is held at
# tauri 2.11.6 (step 2), and the pair moves together in P31a (R20).
js_edits() {
  local f=apps/openalpaca-gui/package.json old new
  while IFS='|' read -r old new; do
    [ -n "$old" ] && edit "$f" "$old" "$new"
  done <<'EOF'
    "@tanstack/react-query": "5.102.8",|    "@tanstack/react-query": "5.104.1",
    "dompurify": "3.4.14",|    "dompurify": "3.4.16",
    "marked": "18.0.11",|    "marked": "18.1.0",
    "react": "19.2.8",|    "react": "19.3.0",
    "react-dom": "19.2.8",|    "react-dom": "19.3.0",
    "tailwind-merge": "3.6.0",|    "tailwind-merge": "3.7.0",
    "@testing-library/dom": "10.4.1",|    "@testing-library/dom": "10.4.2",
    "@testing-library/user-event": "14.6.6",|    "@testing-library/user-event": "14.6.7",
    "@types/node": "26.4.0",|    "@types/node": "26.6.5",
    "@types/react": "19.2.18",|    "@types/react": "19.3.0",
    "@types/react-dom": "19.2.5",|    "@types/react-dom": "19.3.0",
    "@vitejs/plugin-react-swc": "4.3.3",|    "@vitejs/plugin-react": "6.1.2",
    "jsdom": "30.0.1",|    "jsdom": "30.1.2",
    "prettier": "3.9.6",|    "prettier": "3.9.10",
    "vite": "7.3.6",|    "vite": "8.3.4",
    "vitest": "4.1.11"|    "vitest": "5.0.3"
EOF
  edit apps/openalpaca-gui/vite.config.ts 'import react from "@vitejs/plugin-react-swc";' 'import react from "@vitejs/plugin-react";'
}

final_js() {
  CURRENT_STEP="final-js"
  say ""; say "== final: the P31+P32+P33 JS set, a fresh root bun.lock, the four bun gates   ($(now))"
  js_edits
  local -a BUNENV
  BUNENV=(env "BUN_INSTALL=$T/bun-home" "BUN_INSTALL_CACHE_DIR=$T/bun-cache" \
    "BUN_RUNTIME_TRANSPILER_CACHE_PATH=$T/bun-cache/transpiler" "OPENALPACA_HOME_STORE=$T/test-store" \
    "OPENALPACA_CONFIG_DIR=$T/test-store/config" "XDG_CACHE_HOME=$T/xdg-cache" NO_COLOR=1 FORCE_COLOR=0 CI=1)
  if live; then
    mkdir -p "$T/bun-home" "$T/bun-cache" "$T/test-store/config"
    rm -f bun.lock; say "    removed the root bun.lock (P31: written from scratch by the pinned Bun)"
  else
    plan "    rm bun.lock   (the root lock, re-resolved from scratch)"
  fi
  run_logged 92-bun-install "${BUNENV[@]}" "$BUN" install
  live && say "    bun.lock: $(sed -n 's/^ *"lockfileVersion": *\([0-9]*\),*$/lockfileVersion \1/p' bun.lock | head -n 1)"
  cd apps/openalpaca-gui
  run_logged 93-bun-check "${BUNENV[@]}" "$BUN" run check
  run_logged 94-bun-test "${BUNENV[@]}" "$BUN" run test
  run_logged 95-bun-format-check "${BUNENV[@]}" "$BUN" run format:check
  run_logged 96-bun-build "${BUNENV[@]}" "$BUN" run build
  cd "$T/src"
  if live; then
    VITEST_LINE=$(grep -E '^ *(Test Files|Tests) ' "$T/logs/94-bun-test.log" | sed 's/^ *//' | tr '\n' ';' | sed 's/;$//; s/;/; /g')
    say "    vitest: ${VITEST_LINE:-summary lines not found in logs/94-bun-test.log}"
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

summary() {
  local total=$(( LOCK_UNEXPECTED + LOCK_EXPECTED ))
  if [ "$total" -eq 0 ]; then LOCK_LINE="every compared lock equal to its shipped snapshot"
  else LOCK_LINE="$LOCK_UNEXPECTED unexpected, $LOCK_EXPECTED expected (P04 hold / step-back) — $T/logs/lock-differences.txt"; fi
  if [ "$DATES" -eq 1 ] && [ "$total" -gt 0 ]; then
    say ""; say "== release dates of the versions the differences introduced (crates.io; User-Agent openalpaca-deps-check)"
    python3 -I "$HERE/lockdates.py" < "$T/logs/lock-differences.txt" > "$T/logs/lock-dates.txt" 2>&1 || true
    sed 's/^/    /' "$T/logs/lock-dates.txt"
  fi
  say ""
  say "RESULT ($(now)): lock differences: $LOCK_LINE; cargo test: $TEST_LINE; vitest: $VITEST_LINE; G-GUI: $GUI_LINE; logs: $T/logs"
}

# ---------------------------------------------------------------------------- pre-flight
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
  fi
  local f
  for f in lockdiff.py lockdates.py hold-footprint.txt patches/step-4-secrets.patch patches/step-6-rmcp.patch \
           patches/step-8-config.patch locks/recut-lock-00-head.lock locks/recut-lock-01-toolchain.lock \
           locks/recut-lock-02-update-held.lock locks/recut-lock-03-storage.lock locks/recut-lock-04-secrets.lock \
           locks/recut-lock-05-documents.lock locks/recut-lock-06-rmcp.lock locks/recut-lock-07-web.lock \
           locks/recut-lock-99-final-held.lock; do
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
  CURRENT_STEP="setup"
}

# ---------------------------------------------------------------------------- the self-test
self_test() {
  CURRENT_STEP="self-test"
  need perl; need python3; need cargo
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
  CARGO=(nice -n 10 env CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=8 "CARGO_TARGET_DIR=$T/target" cargo "+$TOOLCHAIN")

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

  say ""
  say "SELF-TEST: $pass passed, $fail failed; logs: $T/logs"
  [ "$fail" -eq 0 ] || exit 1
  exit 0
}

# ---------------------------------------------------------------------------- main
main() {
  make_root
  if [ "$SELFTEST" -eq 1 ]; then self_test; fi

  [ -n "$BASE" ] || { echo "gbase: --base <commit> is required (or --self-test)" >&2; usage >&2; exit 2; }
  if [ -z "$REPO" ]; then
    REPO=$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null) || { echo "gbase: $HERE is not inside a repository; pass --repo" >&2; exit 2; }
  fi
  say "repository: $REPO  base: $BASE  toolchain: $TOOLCHAIN  steps: $STEPS  strict: $STRICT  step-backs: $WITH_STEPBACKS  dry-run: $DRY  edits-only: $EDITS_ONLY"
  CARGO=(nice -n 10 env CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=8 "CARGO_TARGET_DIR=$T/target" cargo "+$TOOLCHAIN")
  CARGO_GUI=(nice -n 10 env CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=8 "CARGO_TARGET_DIR=$T/target-gui" cargo "+$TOOLCHAIN")

  preflight

  local last=8
  [ "$STEPS" = all ] || last=$STEPS

  if [ "$DRY" -eq 1 ]; then
    MODE=plan
    plan ""; plan "PLAN (dry run; nothing below was executed) — export of $BASE_SHA, toolchain $TOOLCHAIN"
    run_steps "$last"
    if [ "$STEPS" = all ]; then
      plan ""; plan "== final: cargo test --workspace --exclude openalpaca_gui --no-fail-fast (HOME, OPENALPACA_HOME_STORE, OPENALPACA_CONFIG_DIR under \$T; RUSTUP_HOME/CARGO_HOME real)   > logs/90-test.log"
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
    say ""; say "EDITS-ONLY OK: the eight steps' manifest edits and patches$( [ "$STEPS" = all ] && printf ' and the JS edits') are applied in $T/src; no cargo ran."
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
