#!/bin/bash
# consent-point-test.sh — a re-runnable test of consent-point.sh on synthetic
# stores only. It builds its own fixtures under a fresh `mktemp -d` (in
# ${TMPDIR:-/tmp}) and never reads or names a real store. On the real checkout
# it never passes --include-checkout-config (so the checkout's git-ignored
# config/llm.toml is only stat'ed, never opened); that flag is exercised only
# in a throwaway repository the test creates. Four full runs build every tool
# four times (one of them is interrupted mid-build): a few minutes with a warm
# cargo cache. Exit 0 when every check passes.
#
#   TMPDIR=<a scratch dir> /bin/bash tasks/deps-upgrade/tools/e6/consent-point-test.sh
#
# The fixtures plant three sentinels — SENTINELVALUE in values and file
# bytes, SENTINELKEY in TOML and YAML keys, SENTINELPATH in directory and file
# names and in the homes' own paths — and every check on the two streams is
# that none of them, nor the fixture's path, appears; stdout must end at
# `=== TO HERE ===`, and stderr hold only the line naming the scratch dir. The
# runs:
#   run 1  the fixture home, from its parent with CDPATH=. and a relative
#          --home, under a UTF-8 locale; two skills whose typed word depends on
#          the struct (a duplicated key, a custom tag on an unknown key) show
#          that yamlcmp reads with the base's SkillFrontmatter;
#   run 2  an empty home, --keep, with every child the script closes fds 3/4
#          for shimmed to write to fds 3 and 4, a rustc wrapper set in the
#          environment, MAKEFLAGS naming fds 3 and 4, GIT_TRACE on fd 3;
#   run 3  pathological inputs under --tool-time-limit 10, a FIFO at a path
#          the DU-10 count reads, and an extended attribute set during the
#          builds on a store file no tool reads and on a fed file outside the
#          store (through a link): metadata only, seen by the ctime;
#   run 4  Ctrl-C (SIGINT to the process group) during the builds.
# Before them, runs that stop at the pre-flight or build nothing: usage
# refusals, a closed stdout or stderr, a scratch root that cannot be written,
# a scratch root inside --home's config or plugins through a link, --home /,
# an inherited xtrace / BASH_ENV / DEBUG trap, CDPATH with a relative script
# path, a toolchain not installed, a working tree pinning a toolchain that is
# not installed (the check must not install it: a rustup shim refuses every
# call not made from / with RUSTUP_AUTO_INSTALL=0), a toolchain word outside
# the alphabet under a UTF-8 locale, the block's alphabet filter itself under
# a UTF-8 locale, and the opt-in sources in a throwaway checkout.
#
# The kept build tree of run 2 (about 3.5 GB) is removed by the test; the rest
# of each scratch dir is kept under the fixture directory printed at the end.

# The checks are shell strings run by eval, so their variables expand late.
# shellcheck disable=SC2016,SC2034

set -euo pipefail
export LC_ALL=C

HERE=$(cd -P -- "$(dirname -- "$0")" && pwd -P)
SCRIPT=$HERE/consent-point.sh
R=$(git -C "$HERE" rev-parse --show-toplevel)
unset OPENALPACA_CONFIG_DIR E6_SELFTEST_PANIC CDPATH

FX=$(mktemp -d "${TMPDIR:-/tmp}/e6cpt.XXXXXX")
FX=$(cd -P -- "$FX" && pwd -P)
# The scratch roots of runs 3 and 4, beside the fixture rather than inside it:
# the line after the block names the scratch dir, which must not hold $FX.
SCR=$(mktemp -d "${TMPDIR:-/tmp}/e6cpt-scr.XXXXXX")
SCR=$(cd -P -- "$SCR" && pwd -P)
H=$FX/home-SENTINELPATH
OUTSIDE=$FX/outside-SENTINELPATH
EMPTY=$FX/empty-SENTINELPATH
mkdir -p "$H/config/tools" "$H/config/agents" "$H/config/skills" "$H/config/orchestrator" \
  "$H/plugins/.config" "$H/plugins/p-SENTINELPATH/skills/x" "$OUTSIDE/linked-skill" "$EMPTY"
UNI=$(printf 'u\303\274-SENTINELPATH')   # a skill directory name with a UTF-8 letter

PASS=0
FAILS=0
check() {   # description condition — the condition is a shell string, run by eval
  local d=$1
  if eval "$2"; then
    PASS=$((PASS + 1))
    echo "ok   $d"
  else
    FAILS=$((FAILS + 1))
    echo "FAIL $d"
  fi
}

# ---- the fixture ----------------------------------------------------------------
# llm.toml: a keychain pointer, use_keychain on, an encrypted-looking secret,
# a sentinel key and value.
cat >"$H/config/llm.toml" <<'EOF'
# SENTINELVALUE in a comment
[orchestrator]
model = "SENTINELVALUE-model"

[security]
use_keychain = true

[providers.anthropic]
enabled = true
strategy = "round_robin"
default_model = "SENTINELVALUE"
SENTINELKEY = "SENTINELVALUE"

[[providers.anthropic.keys]]
id = "SENTINELKEY-1"
secret_ref = "llm/anthropic/SENTINELVALUE"

[[providers.anthropic.keys]]
id = "k2"
secret_encrypted = "v1:SENTINELVALUEc2VjcmV0LWJ5dGVz:bm9uY2UtYnl0ZXM="

[providers.ollama]
enabled = false
base_url = "http://127.0.0.1:1/v1"
EOF
# daemon.toml: the checkout's tracked one, with a sentinel key first.
{
  printf 'SENTINELKEY = "SENTINELVALUE"\n'
  git -C "$R" show HEAD:config/daemon.toml
} >"$H/config/daemon.toml"
cat >"$H/config/mcp.toml" <<'EOF'
[defaults]
connect_timeout_secs = 30

[servers.SENTINELKEY]
transport = "stdio"
command = "SENTINELVALUE"
args = ["SENTINELVALUE"]
enabled = false
EOF
cat >"$H/config/tools/x.toml" <<'EOF'
[[tools]]
name = "SENTINELKEY_tool"
description = "SENTINELVALUE"

[tools.backend]
type = "http"
url = "http://127.0.0.1:1/SENTINELVALUE"
method = "GET"
EOF
# The malformed TOML: a parse error on both sides, sentinels in its name and bytes.
printf 'SENTINELKEY = "SENTINELVALUE\n[unclosed\n' >"$H/config/tools/broken-SENTINELPATH.toml"
# A legacy agent TOML and an agent template.
printf '[agent]\nid = "SENTINELVALUE"\nSENTINELKEY = 1\n' >"$H/config/agents/legacy-SENTINELPATH.toml"
{
  git -C "$R" show HEAD:config/agents/general_agent.md
  printf '\nSENTINELVALUE\n'
} >"$H/config/agents/fx-SENTINELPATH.md"
# Skills: a plain one (an unknown sentinel key), the clip-chomping block scalar
# as the last key (yamlcmp `different`), a byte-order mark (`no-frontmatter`,
# typedprobe `err`), a FIFO named SKILL.md (skipped, never opened), a skill
# linked from outside the home (read through the link), one whose directory
# name holds a UTF-8 letter, one in a directory the run cannot enter (mode 000:
# skipped), and two the daemon's one-level scan never loads (flat in skills/,
# and two levels down): counted, not fed.
mkdir -p "$H/config/skills/good" "$H/config/skills/chomp-SENTINELPATH" "$H/config/skills/bom" "$H/config/skills/fifo" \
  "$H/config/skills/$UNI" "$H/config/skills/closed-SENTINELPATH" "$H/config/skills/grp-SENTINELPATH/inner"
printf -- '---\nid: good\nname: Good\ndescription: SENTINELVALUE\nSENTINELKEY: SENTINELVALUE\n---\nBody SENTINELVALUE\n' \
  >"$H/config/skills/good/SKILL.md"
printf -- '---\nname: chomp\ndescription: |\n  SENTINELVALUE first line\n  second line\n---\nBody SENTINELVALUE\n' \
  >"$H/config/skills/chomp-SENTINELPATH/SKILL.md"
printf '\357\273\277---\nname: bom\ndescription: SENTINELVALUE\n---\nBody\n' >"$H/config/skills/bom/SKILL.md"
mkfifo "$H/config/skills/fifo/SKILL.md"
printf -- '---\nname: linked\ndescription: SENTINELVALUE\n---\nBody\n' >"$OUTSIDE/linked-skill/SKILL.md"
ln -s "$OUTSIDE/linked-skill" "$H/config/skills/linked-SENTINELPATH"
printf -- '---\nname: uni\ndescription: SENTINELVALUE\n---\nBody\n' >"$H/config/skills/$UNI/SKILL.md"
printf -- '---\nname: closed\ndescription: SENTINELVALUE\n---\nBody\n' >"$H/config/skills/closed-SENTINELPATH/SKILL.md"
printf -- '---\nname: flat\ndescription: SENTINELVALUE\n---\n' >"$H/config/skills/SKILL.md"
# Two skills whose typed word depends on SkillFrontmatter's shape: with
# N-SKILL-2's `#[serde(flatten)] unknown_keys` a duplicated top-level key is
# refused by both readers (`parse-error-both`), and a custom tag on an unknown
# key by serde_yaml alone (`parse-error-old`); a struct copy from before the
# flatten reads them `parse-error-old` and `equal`.
mkdir -p "$H/config/skills/dup-SENTINELPATH" "$H/config/skills/tag-SENTINELPATH"
printf -- '---\nname: dup\ndescription: SENTINELVALUE one\ndescription: SENTINELVALUE two\n---\nBody\n' \
  >"$H/config/skills/dup-SENTINELPATH/SKILL.md"
printf -- '---\nname: tag\ndescription: d\nSENTINELKEY: !custom SENTINELVALUE\n---\nBody\n' \
  >"$H/config/skills/tag-SENTINELPATH/SKILL.md"
printf -- '---\nname: deep\ndescription: SENTINELVALUE\n---\n' >"$H/config/skills/grp-SENTINELPATH/inner/SKILL.md"
# The four persona documents, from the tracked templates.
for p in SOUL USER IDENTITY BOOTSTRAP; do
  {
    git -C "$R" show "HEAD:config/orchestrator/templates/${p}_temp.md"
    printf '\nSENTINELVALUE\n'
  } >"$H/config/orchestrator/$p.md"
done
# Plugins: a manifest, a SKILL.md inside the plugin (counted, not fed), the
# disposition store, the plugin config's inline secret_ref, and a plugin
# directory the run cannot enter (mode 000: skipped).
cat >"$H/plugins/p-SENTINELPATH/plugin.toml" <<'EOF'
[plugin]
name = "p-sentinel"
version = "0.1.0"
description = "SENTINELVALUE"
entry = "./SENTINELVALUE.sh"

[types]
tools = true
EOF
printf -- '---\nname: plug\ndescription: SENTINELVALUE\n---\n' >"$H/plugins/p-SENTINELPATH/skills/x/SKILL.md"
cat >"$H/plugins/.permissions.toml" <<'EOF'
[plugins.p-sentinel]
approved = true
enabled = true
capabilities = ["SENTINELVALUE"]
EOF
cat >"$H/plugins/.config/p-SENTINELPATH.toml" <<'EOF'
SENTINELKEY = { secret_ref = "openalpaca-plugin-p-sentinel-SENTINELVALUE" }
plain = "SENTINELVALUE"
EOF
mkdir -p "$H/plugins/closed-SENTINELPATH"
printf '[plugin]\nname = "closed"\ndescription = "SENTINELVALUE"\n' >"$H/plugins/closed-SENTINELPATH/plugin.toml"
CLOSED=("$H/config/skills/closed-SENTINELPATH" "$H/plugins/closed-SENTINELPATH")
chmod 000 "${CLOSED[@]}"
# A scratch root the run cannot write (mode 555), for the mktemp refusal.
RO=$FX/ro-SENTINELPATH
mkdir -p "$RO"
chmod 555 "$RO"
reopen() { chmod 755 "${CLOSED[@]}" "$RO" 2>/dev/null || true; }
trap reopen EXIT

# An independent proof that nothing in the fixture changed: names, types,
# modes, mtimes and hashes of the home and of the directory its link points
# into (find's error on each mode-000 directory is part of the state).
fixture_state() {
  (cd "$FX" && {
    find home-SENTINELPATH outside-SENTINELPATH empty-SENTINELPATH -print 2>&1 || true
    find home-SENTINELPATH outside-SENTINELPATH -type f -exec shasum -a 256 {} + 2>&1 || true
    find home-SENTINELPATH outside-SENTINELPATH -exec stat -f '%N %HT %Lp %m' {} + 2>&1 || true
  } | LC_ALL=C sort)
}
fixture_state >"$FX/fixture.before"
CB_BEFORE=$({ find "${CARGO_HOME:-$HOME/.cargo}/build" -mindepth 1 -maxdepth 1 2>/dev/null || true; } | wc -l | tr -d ' ')

# ---- helpers on a run's output ---------------------------------------------------
block_of() { awk '/^=== PASTE FROM HERE ===$/{p=1; next} /^=== TO HERE ===$/{p=0} p' "$1"; }
# The scratch dir, from the line after the block — on stderr.
scratch_of() { sed -n 's/^(consent-point, not for pasting: \(.*\) holds the index lists that map each index to its file, .*$/\1/p' "$1"; }
# Stdout is the block alone: it starts at the first frame and ends at the second.
stdout_is_block() { [ "$(head -n 1 "$1")" = "=== PASTE FROM HERE ===" ] && [ "$(tail -n 1 "$1")" = "=== TO HERE ===" ]; }
# Stderr is the one line naming the scratch dir, and nothing else.
stderr_is_trailer() { [ "$(wc -l <"$1" | tr -d ' ')" = 1 ] && [ -n "$(scratch_of "$1")" ]; }
no_leak() {   # file...
  local f
  for f in "$@"; do
    grep -q -F -e SENTINELVALUE -e SENTINELKEY -e SENTINELPATH -e "$FX" "$f" && return 1
  done
  return 0
}
has_line() { grep -q -x -F -- "$2" "$1"; }
has_match() { grep -q -E -- "$2" "$1"; }
alphabet_ok() { ! grep -q -e '/' -e '[^A-Za-z0-9 ._:=(),+-]' "$1"; }
no_high_byte() { ! LC_ALL=C grep -q "$(printf '[\200-\377]')" "$1"; }
index_of() {   # list-file path-suffix
  awk -F '\t' -v s="$2" 'substr($3, length($3) - length(s) + 1) == s { print $1 }' "$1"
}
summary_value() {   # block label key
  awk -v l="$2" -v k="$3" '$1 == l { for (i = 2; i <= NF; i++) { split($i, a, "="); if (a[1] == k) print a[2] } }' "$1"
}
no_process_of() { ! pgrep -f -- "$1" >/dev/null 2>&1; }   # scratch dir

# ---- usage refusals (no build) ---------------------------------------------------
rc=0
/bin/bash "$SCRIPT" >"$FX/u1.out" 2>"$FX/u1.err" || rc=$?
check "no --home: exit 2" '[ "$rc" = 2 ]'
rc=0
/bin/bash "$SCRIPT" --home "$FX/absent-SENTINELPATH" >"$FX/u2.out" 2>"$FX/u2.err" || rc=$?
check "a --home that does not exist: exit 2" '[ "$rc" = 2 ]'
rc=0
/bin/bash "$SCRIPT" --home "$H" --SENTINELPATH >"$FX/u3.out" 2>"$FX/u3.err" || rc=$?
check "an unknown argument: exit 2" '[ "$rc" = 2 ]'
rc=0
/bin/bash "$SCRIPT" --home "$H" --tool-time-limit 0 >"$FX/u4.out" 2>"$FX/u4.err" || rc=$?
check "a zero time limit: exit 2" '[ "$rc" = 2 ]'
rc=0
/bin/bash "$SCRIPT" --home "$H" --tool-time-limit 5x >"$FX/u5.out" 2>"$FX/u5.err" || rc=$?
check "a time limit that is not a number: exit 2" '[ "$rc" = 2 ]'
check "the refusals print no sentinel and no fixture path" 'no_leak "$FX"/u[1-5].*'
check "the refusals print nothing on stdout" '[ ! -s "$FX/u1.out" ] && [ ! -s "$FX/u2.out" ] && [ ! -s "$FX/u3.out" ] && [ ! -s "$FX/u4.out" ] && [ ! -s "$FX/u5.out" ]'

# ---- the scratch root and the terminal's streams (no build) -------------------------
# A closed stdout or stderr, and a scratch root mktemp cannot write: exit 2 with
# the fixed text only (mktemp's own error names the root), and no scratch dir
# left behind.
TC1=$FX/tmpc-SENTINELPATH
mkdir -p "$TC1"
rc=0
env TMPDIR="$TC1" /bin/bash "$SCRIPT" --home "$H" >&- 2>"$FX/p1.err" || rc=$?
check "stdout closed: exit 2, the fixed text, no scratch dir left" '[ "$rc" = 2 ] && grep -q -x -F "consent-point: stdout or stderr is closed" "$FX/p1.err" && [ -z "$(ls -A "$TC1")" ]'
rc=0
env TMPDIR="$TC1" /bin/bash "$SCRIPT" --home "$H" >"$FX/p2.out" 2>&- || rc=$?
check "stderr closed: exit 2, nothing on stdout, no scratch dir left" '[ "$rc" = 2 ] && [ ! -s "$FX/p2.out" ] && [ -z "$(ls -A "$TC1")" ]'
rc=0
env TMPDIR="$RO" /bin/bash "$SCRIPT" --home "$H" >"$FX/p3.out" 2>"$FX/p3.err" || rc=$?
check "a scratch root mktemp cannot write: exit 2, the fixed text only" '[ "$rc" = 2 ] && grep -q -x -F "consent-point: mktemp failed under the scratch root" "$FX/p3.err" && ! grep -q "mktemp:" "$FX/p3.err" && [ "$(wc -l <"$FX/p3.err" | tr -d " ")" = 2 ] && [ ! -s "$FX/p3.out" ]'
# --home's config and plugins as links to directories that hold the scratch
# root: the scratch dir would be a write into the store through a link.
LK=$FX/lnk-SENTINELPATH
mkdir -p "$LK/home" "$LK/realcfg/tmp" "$LK/realplug/tmp"
ln -s "$LK/realcfg" "$LK/home/config"
ln -s "$LK/realplug" "$LK/home/plugins"
rc=0
env TMPDIR="$LK/realcfg/tmp" /bin/bash "$SCRIPT" --home "$LK/home" --base no-such-ref-e6 >"$FX/p4.out" 2>"$FX/p4.err" || rc=$?
check "a scratch root inside --home's config through a link: refused before --base is read" '[ "$rc" = 2 ] && grep -q -F "is inside --home'\''s config (through a link)" "$FX/p4.err" && [ -z "$(ls -A "$LK/realcfg/tmp")" ]'
rc=0
env TMPDIR="$LK/realplug/tmp" /bin/bash "$SCRIPT" --home "$LK/home" --base no-such-ref-e6 >"$FX/p5.out" 2>"$FX/p5.err" || rc=$?
check "a scratch root inside --home's plugins through a link: refused" '[ "$rc" = 2 ] && grep -q -F "is inside --home'\''s plugins (through a link)" "$FX/p5.err" && [ -z "$(ls -A "$LK/realplug/tmp")" ]'
rc=0
env TMPDIR="$TC1" /bin/bash "$SCRIPT" --home / --base no-such-ref-e6 >"$FX/p6.out" 2>"$FX/p6.err" || rc=$?
check "--home / holds every scratch root: refused" '[ "$rc" = 2 ] && grep -q -F "is inside --home; set TMPDIR elsewhere" "$FX/p6.err" && [ -z "$(ls -A "$TC1")" ]'
check "these refusals print no sentinel and no fixture path, and nothing on stdout" 'no_leak "$FX"/p[1-6].* && [ ! -s "$FX/p4.out" ] && [ ! -s "$FX/p5.out" ] && [ ! -s "$FX/p6.out" ]'

# ---- an inherited xtrace, BASH_ENV and DEBUG trap (no build) -----------------------
# The trace of any line that holds an argument would name --home on the
# terminal's stderr. --base names no commit, so each stops at the pre-flight.
printf 'set -x\ntrap '\''echo "DEBUG $BASH_COMMAND" >&2'\'' DEBUG\n' >"$FX/bashenv.sh"
rc=0
env SHELLOPTS=xtrace /bin/bash "$SCRIPT" --home "$H" --base no-such-ref-e6 >"$FX/x1.out" 2>"$FX/x1.err" || rc=$?
check "xtrace from SHELLOPTS: exit 2, the --base refusal" '[ "$rc" = 2 ] && grep -q -F -- "--base does not name a commit" "$FX/x1.err"'
rc=0
env BASH_ENV="$FX/bashenv.sh" /bin/bash "$SCRIPT" --home "$H" --base no-such-ref-e6 >"$FX/x2.out" 2>"$FX/x2.err" || rc=$?
check "xtrace and a DEBUG trap from BASH_ENV: exit 2, the --base refusal" '[ "$rc" = 2 ] && grep -q -F -- "--base does not name a commit" "$FX/x2.err"'
rc=0
env SHELLOPTS=xtrace BASH_ENV="$FX/bashenv.sh" ENV="$FX/bashenv.sh" /bin/sh "$SCRIPT" --home "$H" --base no-such-ref-e6 >"$FX/x3.out" 2>"$FX/x3.err" || rc=$?
check "under /bin/sh with xtrace (the re-exec): exit 2, the --base refusal" '[ "$rc" = 2 ] && grep -q -F -- "--base does not name a commit" "$FX/x3.err"'
check "an inherited xtrace or DEBUG trap prints no sentinel and no fixture path" 'no_leak "$FX"/x[1-3].*'

# ---- CDPATH with a relative script path (no build) --------------------------------
# An exported CDPATH made `cd` print the directory it found, so the script's
# own directory read as two lines; the run must reach the --wave refusal.
rc=0
(cd "$R" && env CDPATH=".:$FX" /bin/bash tasks/deps-upgrade/tools/e6/consent-point.sh --home "$H" --wave refs/prep/no-such-ref-e6 \
  >"$FX/c1.out" 2>"$FX/c1.err") || rc=$?
check "CDPATH and a relative script path: exit 2 at the --wave refusal" '[ "$rc" = 2 ] && grep -q -F -- "--wave does not name a commit" "$FX/c1.err"'
check "CDPATH: no sentinel and no fixture path" 'no_leak "$FX/c1.out" "$FX/c1.err"'

# ---- the block's alphabet filter under a UTF-8 locale (no build) -------------------
# The filter's own lines, taken from the script, run where the caller's locale
# is en_US.UTF-8: every line holding a byte outside the alphabet is dropped,
# UTF-8 letters among them. The control shows that the same pattern without
# the function's LC_ALL=C keeps those letters in this locale.
eval "$(sed -n '/^# >>> block_filter/,/^# <<< block_filter/p' "$SCRIPT")"
printf '%b\n' 'plain words ok=1' 'a\303\274b' 'a\303\251b' 'a\303\205b' 'a\303\237b' 'a\302\252b' 'a\302\272b' \
  'a\346\227\245b' 'a/b' 'a\tb' 'step-failed run-x-timeout' >"$FX/filter.in"
# shellcheck disable=SC2030,SC2031  # the locale is meant to stay inside the subshell
(export LC_ALL=en_US.UTF-8; block_filter <"$FX/filter.in") >"$FX/filter.out"
printf '%s\n' 'plain words ok=1' 'step-failed run-x-timeout' \
  "dropped-lines 9 (outside the block's alphabet; nothing of them printed)" >"$FX/filter.want"
check "the filter under en_US.UTF-8 keeps the two clean lines and drops nine" 'cmp -s "$FX/filter.out" "$FX/filter.want"'
check "the filter's output holds no byte above 127" 'no_high_byte "$FX/filter.out"'
control_g() {   # the filter's pattern without the function's LC_ALL=C
  case "$1" in
    *[!A-Za-z0-9\ ._:=\(\),+-]*) echo dropped ;;
    *) echo kept ;;
  esac
}
# shellcheck disable=SC2030,SC2031  # the locale is meant to stay inside the subshell
control=$(export LC_ALL=en_US.UTF-8; control_g "$(printf 'a\303\274b')")
check "control: without LC_ALL=C the pattern keeps a UTF-8 letter in this locale" '[ "$control" = kept ]'

# ---- a toolchain that is not installed (no build, nothing read) ------------------
# A throwaway repository holding only the script and a rust-toolchain.toml
# naming a toolchain no machine has: the run must stop at its pre-flight, with
# the block and one stderr line, before it exports, builds or lists anything.
mkrepo() {   # dir channel
  mkdir -p "$1/tasks/deps-upgrade/tools/e6"
  cp "$SCRIPT" "$1/tasks/deps-upgrade/tools/e6/consent-point.sh"
  printf '[toolchain]\nchannel = "%s"\n' "$2" >"$1/rust-toolchain.toml"
  git -C "$1" init -q
  git -C "$1" add -A
  git -C "$1" -c user.name=e6-test -c user.email=e6-test@invalid -c commit.gpgsign=false -c core.hooksPath=/dev/null commit -q -m fixture
}
TR=$FX/tcrepo
mkrepo "$TR" 0.0.1
rc=0
/bin/bash "$TR/tasks/deps-upgrade/tools/e6/consent-point.sh" --home "$H" --wave HEAD >"$FX/tc.out" 2>"$FX/tc.err" || rc=$?
block_of "$FX/tc.out" >"$FX/tc.block"
XT=$(scratch_of "$FX/tc.err")
check "missing toolchain: exit 1" '[ "$rc" = 1 ]'
check "missing toolchain: the block names the step and the toolchain" 'has_line "$FX/tc.block" "step-failed toolchain-check" && has_line "$FX/tc.block" "toolchain-missing 0.0.1" && has_line "$FX/tc.block" "exit 1"'
check "missing toolchain: stdout is the block alone" 'stdout_is_block "$FX/tc.out"'
check "missing toolchain: two stderr lines, the one naming it, then the scratch dir" '[ "$(wc -l <"$FX/tc.err" | tr -d " ")" = 2 ] && head -n 1 "$FX/tc.err" | grep -q -F "toolchain(s) 0.0.1 not installed" && [ -n "$XT" ]'
check "missing toolchain: no sentinel and no fixture path on either stream" 'no_leak "$FX/tc.out" "$FX/tc.err"'
check "missing toolchain: nothing exported, built or listed; the empty build tree removed" '[ -n "$XT" ] && [ ! -e "$XT/build" ] && [ -z "$(ls -A "$XT/bin")" ] && [ -z "$(ls -A "$XT/lists")" ]'

# A toolchain word holding a UTF-8 letter, under a UTF-8 locale: the word check
# refuses it byte by byte, and no byte of it reaches either stream.
TR2=$FX/tcrepo2
mkrepo "$TR2" "$(printf '0.0.1\303\274')"
rc=0
env LC_ALL=en_US.UTF-8 LANG=en_US.UTF-8 /bin/bash "$TR2/tasks/deps-upgrade/tools/e6/consent-point.sh" --home "$H" --wave HEAD \
  >"$FX/tc2.out" 2>"$FX/tc2.err" || rc=$?
block_of "$FX/tc2.out" >"$FX/tc2.block"
check "a toolchain word with a UTF-8 letter: exit 1, toolchain-read, the word not printed" '[ "$rc" = 1 ] && has_line "$FX/tc2.block" "step-failed toolchain-read" && has_match "$FX/tc2.block" "^base [0-9a-f]{40} toolchain unreadable$"'
check "a toolchain word with a UTF-8 letter: no byte above 127 in the block or on stderr" 'no_high_byte "$FX/tc2.block" && no_high_byte "$FX/tc2.err"'
check "a toolchain word with a UTF-8 letter: the block keeps to its alphabet" 'alphabet_ok "$FX/tc2.block"'

# A working tree pinning a toolchain that is not installed, and RUSTUP_TOOLCHAIN
# naming it too: rustup 1.29's `rustup toolchain list` installs the active
# toolchain when it is missing, and the active one comes from the cwd's
# rust-toolchain.toml or from RUSTUP_TOOLCHAIN. The committed pin is the
# checkout's (installed), so the pre-flight must pass; a rustup shim first on
# PATH logs each call and runs the real rustup only when it is called from /
# with RUSTUP_AUTO_INSTALL=0 and no RUSTUP_TOOLCHAIN — any other call is
# refused, so nothing can be installed whatever the script does. A cargo shim
# refuses every call. The export holds no e6 tools, so nothing is built.
TC_HEAD=$(git -C "$R" show HEAD:rust-toolchain.toml | sed -n 's/^channel *= *"\(.*\)"$/\1/p' | head -n 1)
TC_ABSENT=1.80.0
TR3=$FX/tcrepo3
mkrepo "$TR3" "$TC_HEAD"
printf '[toolchain]\nchannel = "%s"\n' "$TC_ABSENT" >"$TR3/rust-toolchain.toml"   # uncommitted
SH3=$FX/shim-rustup
mkdir -p "$SH3"
REAL_RUSTUP=$(command -v rustup)
cat >"$SH3/rustup" <<EOF
#!/bin/sh
here=\$(pwd -P)
echo "rustup cwd=\$here auto=\${RUSTUP_AUTO_INSTALL-unset} tc=\${RUSTUP_TOOLCHAIN-unset} args=\$*" >>"\${E6_SHIM_LOG:?}"
if [ "\$here" != / ] || [ "\${RUSTUP_AUTO_INSTALL-unset}" != 0 ] || [ -n "\${RUSTUP_TOOLCHAIN+x}" ]; then
  echo refused >>"\$E6_SHIM_LOG"
  exit 1
fi
exec "$REAL_RUSTUP" "\$@"
EOF
printf '#!/bin/sh\necho "cargo args=$*" >>"${E6_SHIM_LOG:?}"\nexit 1\n' >"$SH3/cargo"
chmod +x "$SH3/rustup" "$SH3/cargo"
# The control: the same shim, called the way the old check called it (from the
# repository, with nothing set), refuses.
rc=0
(cd "$TR3" && env E6_SHIM_LOG="$FX/shim-control.log" "$SH3/rustup" toolchain list >/dev/null 2>&1) || rc=$?
check "control: the shim refuses a call from a repository whose pin is not installed" '[ "$rc" = 1 ] && grep -q -x refused "$FX/shim-control.log"'
check "the absent channel is really not installed" '! (cd / && env -u RUSTUP_TOOLCHAIN RUSTUP_AUTO_INSTALL=0 "$REAL_RUSTUP" toolchain list 2>/dev/null) | grep -q "^$TC_ABSENT"'
rc=0
(cd "$TR3" && env PATH="$SH3:$PATH" E6_SHIM_LOG="$FX/shim3.log" RUSTUP_TOOLCHAIN="$TC_ABSENT" \
  /bin/bash "$TR3/tasks/deps-upgrade/tools/e6/consent-point.sh" --home "$H" --wave HEAD >"$FX/tc3.out" 2>"$FX/tc3.err") || rc=$?
block_of "$FX/tc3.out" >"$FX/tc3.block"
check "a working-tree pin not installed: the pre-flight passed (no toolchain-missing), the run went on" '[ "$rc" = 1 ] && ! grep -q -e "^toolchain-missing" -e "^step-failed toolchain" "$FX/tc3.block" && has_line "$FX/tc3.block" "step-failed export-base-has-no-e6-tools"'
check "a working-tree pin not installed: every rustup call from /, with RUSTUP_AUTO_INSTALL=0 and no RUSTUP_TOOLCHAIN" '[ "$(grep -c -x "rustup cwd=/ auto=0 tc=unset args=toolchain list" "$FX/shim3.log")" = 2 ] && [ "$(wc -l <"$FX/shim3.log" | tr -d " ")" = 2 ]'
check "a working-tree pin not installed: no sentinel and no fixture path on either stream" 'no_leak "$FX/tc3.out" "$FX/tc3.err" && stdout_is_block "$FX/tc3.out" && stderr_is_trailer "$FX/tc3.err"'

# ---- the opt-in sources, in a throwaway checkout (no build) ------------------------
# --include-checkout-config and $OPENALPACA_CONFIG_DIR, exercised where the
# "checkout" is a throwaway repository: the script resolves its checkout from
# its own location, so the real checkout's config/ is out of reach by
# construction. The export holds no e6 tools, so every build step fails at
# once (no cargo) and the list phase still runs; the block's list lines are
# what this checks.
OR=$FX/optrepo
EXTRA=$FX/extra-SENTINELPATH
mkdir -p "$OR/tasks/deps-upgrade/tools/e6" "$OR/config/orchestrator" "$OR/config/tools" "$EXTRA/orchestrator"
cp "$SCRIPT" "$OR/tasks/deps-upgrade/tools/e6/consent-point.sh"
git -C "$R" show HEAD:rust-toolchain.toml >"$OR/rust-toolchain.toml"
printf '# tracked\n' >"$OR/config/orchestrator/USER.md"
printf 'tracked = 1\n' >"$OR/config/tools/t.toml"
git -C "$OR" init -q
git -C "$OR" add -A
git -C "$OR" -c user.name=e6-test -c user.email=e6-test@invalid -c commit.gpgsign=false -c core.hooksPath=/dev/null commit -q -m fixture
# After the commit: the seeded llm.toml and a generated SOUL.md (untracked), a
# skill nobody committed (untracked), and the tracked tools TOML modified.
printf '[[providers.a.keys]]\nid = "SENTINELKEY"\nsecret_ref = "SENTINELVALUE"\n' >"$OR/config/llm.toml"
printf -- '---\ntitle: SENTINELVALUE\n---\n# SOUL\n' >"$OR/config/orchestrator/SOUL.md"
mkdir -p "$OR/config/skills/u-SENTINELPATH"
printf -- '---\nname: u\ndescription: SENTINELVALUE\n---\n' >"$OR/config/skills/u-SENTINELPATH/SKILL.md"
printf 'SENTINELKEY = "SENTINELVALUE"\n' >"$OR/config/tools/t.toml"
printf 'SENTINELKEY = "SENTINELVALUE"\n' >"$EXTRA/llm.toml"
printf -- '---\ntitle: SENTINELVALUE\n---\n# SOUL\n' >"$EXTRA/orchestrator/SOUL.md"
optrun() {   # name config-dir extra-args...
  local name=$1 cfg=$2
  shift 2
  rc=0
  OPENALPACA_CONFIG_DIR=$cfg /bin/bash "$OR/tasks/deps-upgrade/tools/e6/consent-point.sh" --home "$H" --wave HEAD ${1+"$@"} \
    >"$FX/$name.out" 2>"$FX/$name.err" || rc=$?
  block_of "$FX/$name.out" >"$FX/$name.block"
}
optrun opt1 "$EXTRA" --include-checkout-config
check "opt-in: exit 1 (no tools in the throwaway export), the block printed" '[ "$rc" = 1 ] && has_line "$FX/opt1.block" "exit 1" && has_line "$FX/opt1.block" "step-failed export-base-has-no-e6-tools"'
check "opt-in: no sentinel and no fixture path on either stream" 'no_leak "$FX/opt1.out" "$FX/opt1.err"'
check "opt-in: the block keeps to its alphabet" 'alphabet_ok "$FX/opt1.block"'
check "opt-in: the checkout's llm.toml present and included" 'has_line "$FX/opt1.block" "checkout-llm-toml present=yes included=yes"'
check "opt-in: four checkout files not the export's (llm.toml, SOUL.md, the untracked skill, the modified TOML), fed" 'has_line "$FX/opt1.block" "checkout-config-local files=4 included=yes"'
check "opt-in: the config dir included" 'has_line "$FX/opt1.block" "openalpaca-config-dir included"'
check "opt-in: llm list = home, config dir, checkout-local" 'has_line "$FX/opt1.block" "list llm inputs=3 home=1 extra=1 checkout=0 checkout-local=1 missing=0 skipped=0"'
check "opt-in: SOUL from home, config dir and the untracked checkout copy" 'has_line "$FX/opt1.block" "list tp-soul inputs=3 home=1 extra=1 checkout=0 checkout-local=1 missing=0 skipped=0"'
check "opt-in: the tracked USER.md is the export's, not a checkout-local one" 'has_line "$FX/opt1.block" "list tp-user inputs=2 home=1 extra=0 checkout=1 checkout-local=0 missing=1 skipped=0"'
check "opt-in: the untracked skill is fed as checkout-local" 'has_match "$FX/opt1.block" "^list skills inputs=[0-9]+ home=7 extra=0 checkout=0 checkout-local=1 "'
check "opt-in: the modified tracked TOML is fed from both the export and the working tree" 'has_match "$FX/opt1.block" "^list toml inputs=[0-9]+ home=[0-9]+ extra=1 checkout=1 checkout-local=2 "'
check "opt-in: the waiver names the failed keycount step" 'has_line "$FX/opt1.block" "P12-keychain-waiver does-not-hold (keycount-step-failed)"'
check "opt-in: store-unchanged yes, inputs-unchanged yes" 'has_line "$FX/opt1.block" "store-unchanged yes" && has_line "$FX/opt1.block" "inputs-unchanged yes"'
optrun opt2 "$EXTRA"
check "without the flag: the checkout's llm.toml present, not included" 'has_line "$FX/opt2.block" "checkout-llm-toml present=yes included=no"'
check "without the flag: the four checkout files counted, not fed" 'has_line "$FX/opt2.block" "checkout-config-local files=4 included=no" && has_match "$FX/opt2.block" "^list skills inputs=[0-9]+ home=7 extra=0 checkout=0 checkout-local=0 "'
check "without the flag: the config dir with an llm.toml is named, not read" 'has_line "$FX/opt2.block" "openalpaca-config-dir not-included-has-llm" && has_line "$FX/opt2.block" "list llm inputs=1 home=1 extra=0 checkout=0 checkout-local=0 missing=0 skipped=0"'
check "without the flag: no sentinel and no fixture path" 'no_leak "$FX/opt2.out" "$FX/opt2.err"'
optrun opt3 "$OR/config" --include-checkout-config
check "a config dir that is the checkout's: same-as-checkout, read once" 'has_line "$FX/opt3.block" "openalpaca-config-dir same-as-checkout" && has_line "$FX/opt3.block" "list llm inputs=2 home=1 extra=0 checkout=0 checkout-local=1 missing=0 skipped=0"'
check "a config dir that is the checkout's: no sentinel and no fixture path" 'no_leak "$FX/opt3.out" "$FX/opt3.err"'

# ---- run 1: the fixture home ------------------------------------------------------
# From the home's parent, with CDPATH=. and a relative --home (canon must give
# one line), under a UTF-8 locale (the script's own LC_ALL=C must hold).
t0=$SECONDS
rc=0
(cd "$FX" && env CDPATH=. LC_ALL=en_US.UTF-8 LANG=en_US.UTF-8 /bin/bash "$SCRIPT" --home home-SENTINELPATH \
  >"$FX/run1.out" 2>"$FX/run1.err") || rc=$?
echo "run 1 took $((SECONDS - t0)) s, exit $rc"
block_of "$FX/run1.out" >"$FX/run1.block"
X1=$(scratch_of "$FX/run1.err")
check "run 1: exit 0" '[ "$rc" = 0 ]'
check "run 1: stderr is the one line naming the scratch dir" 'stderr_is_trailer "$FX/run1.err"'
check "run 1: stdout and stderr hold no sentinel and no fixture path" 'no_leak "$FX/run1.out" "$FX/run1.err"'
check "run 1: one frame of each" '[ "$(grep -c -x '\''=== PASTE FROM HERE ==='\'' "$FX/run1.out")" = 1 ] && [ "$(grep -c -x '\''=== TO HERE ==='\'' "$FX/run1.out")" = 1 ]'
check "run 1: stdout is the block alone (nothing before it, nothing after it)" 'stdout_is_block "$FX/run1.out"'
check "run 1: stdout holds no '/' at all (no path)" '! grep -q / "$FX/run1.out"'
check "run 1: the scratch dir the stderr line names exists" '[ -n "$X1" ] && [ -d "$X1" ]'
check "run 1: the block keeps to its alphabet (no '/')" 'alphabet_ok "$FX/run1.block"'
check "run 1: no line was dropped" '! grep -q "^dropped-lines" "$FX/run1.block"'
check "run 1: status complete, exit 0" 'has_line "$FX/run1.block" "status complete"'
check "run 1: the block says exit 0" 'has_line "$FX/run1.block" "exit 0"'
check "run 1: no step failed" '! grep -q '\''^step-failed'\'' "$FX/run1.block"'
check "run 1: store-unchanged yes" 'has_line "$FX/run1.block" "store-unchanged yes"'
check "run 1: inputs-unchanged yes" 'has_line "$FX/run1.block" "inputs-unchanged yes"'
check "run 1: the build tree was removed (no --keep)" '[ ! -e "$X1/build" ]'
check "run 1: the index lists are kept" '[ -s "$X1/lists/skills.list" ] && [ -s "$X1/lists/toml.list" ]'
check "run 1: the base runs the old sides' pins" 'has_line "$FX/run1.block" "base-matches-old-pins toml=yes toml_edit=yes serde_yaml=yes"'
check "run 1: the wave lock line names serde-saphyr (1.3.0 on the default --wave)" 'has_match "$FX/run1.block" "^typedprobe-wave-lock moved=0 .* serde-saphyr=1\.3\.0 granit-parser=1\.3\.0$"'
check "run 1: wave-matches-new-pins: toml and toml_edit the cut'\''s, serde-saphyr not" 'has_match "$FX/run1.block" "^wave-matches-new-pins toml=yes toml_edit=yes serde-saphyr=no "'
check "run 1: yamlcmp ran on the base'\''s struct (the committed copy may lag: yes or no)" 'has_match "$FX/run1.block" "^yamlcmp-struct source=base committed-copy-matches-base (yes|no)$"'
# The default --wave predates N-SKILL-2 (8c63804f), so the base changed the
# skill loader since the two parted.
check "run 1: wave-loader-behind-base names every kind, and the skill loader as changed" 'has_match "$FX/run1.block" "^wave-loader-behind-base files=[1-9][0-9]* skill=yes persona=(yes|no) agent=(yes|no) daemon-config=(yes|no) mcp-config=(yes|no) llm-config=(yes|no) plugin-manifest=(yes|no)$"'
check "run 1: old-side-stack-matches-base for tomlcmp, tedit-old and keycount" '[ "$(grep -c -E "^old-side-stack-matches-base (tomlcmp|tedit-old|keycount) toml_parser=(yes|no|absent) toml_writer=(yes|no|absent) toml_datetime=(yes|no|absent) serde_spanned=(yes|no|absent) indexmap=(yes|no|absent) winnow=(yes|no|absent) \(informational\)$" "$FX/run1.block")" = 3 ]'
check "run 1: keycount'\''s lock is the base'\''s TOML stack" 'has_line "$FX/run1.block" "old-side-stack-matches-base keycount toml_parser=yes toml_writer=yes toml_datetime=yes serde_spanned=yes indexmap=yes winnow=yes (informational)"'

SK=$X1/lists/skills.list
TL=$X1/lists/toml.list
I_CHOMP=$(index_of "$SK" "/chomp-SENTINELPATH/SKILL.md")
I_BOM=$(index_of "$SK" "/bom/SKILL.md")
I_GOOD=$(index_of "$SK" "/good/SKILL.md")
I_LINK=$(index_of "$SK" "/linked-SENTINELPATH/SKILL.md")
I_UNI=$(index_of "$SK" "/$UNI/SKILL.md")
I_DUP=$(index_of "$SK" "/dup-SENTINELPATH/SKILL.md")
I_TAG=$(index_of "$SK" "/tag-SENTINELPATH/SKILL.md")
I_BROKEN=$(index_of "$TL" "/broken-SENTINELPATH.toml")
check "run 1: the fixture's skills and the malformed TOML are in the lists" '[ -n "$I_CHOMP" ] && [ -n "$I_BOM" ] && [ -n "$I_GOOD" ] && [ -n "$I_LINK" ] && [ -n "$I_UNI" ] && [ -n "$I_DUP" ] && [ -n "$I_TAG" ] && [ -n "$I_BROKEN" ]'
check "run 1: the lists hold the home's canonical path (the relative --home resolved once)" 'awk -F "\t" -v p="$H/config/skills/good/SKILL.md" '\''$2 == "home" && $3 == p { f = 1 } END { exit !f }'\'' "$SK"'
check "run 1: the FIFO SKILL.md, the closed dir and the unscanned ones are not in the skill list" '! grep -q -E '\''/(fifo|closed-SENTINELPATH|grp-SENTINELPATH/inner)/SKILL.md$|/skills/SKILL.md$'\'' "$SK"'
check "run 1: the plugin's own SKILL.md is not in the skill list" '! grep -q '\''/p-SENTINELPATH/skills/'\'' "$SK"'
check "run 1: the skill list: seven from home, the FIFO and the closed dir skipped" 'has_match "$FX/run1.block" '\''^list skills inputs=[0-9]+ home=7 .* skipped=2$'\'''
check "run 1: the closed plugin dir is skipped, the open one fed" 'has_match "$FX/run1.block" '\''^list tp-plugin inputs=1 home=1 .* skipped=1$'\'''
check "run 1: the two SKILL.md the scan never loads are counted, not fed" 'has_line "$FX/run1.block" "skill-md-not-loaded-by-scan 2 (not fed)"'
check "run 1: the plugin dir's SKILL.md is counted, not fed" 'has_match "$FX/run1.block" '\''^skill-md-under-plugin-dirs 1 '\'''
check "run 1: yamlcmp reads the clip-chomping skill 'different'" 'has_line "$FX/run1.block" "yamlcmp $I_CHOMP different"'
check "run 1: yamlcmp's value facet reads it 'different' too" 'has_line "$FX/run1.block" "yamlcmp-value $I_CHOMP different"'
check "run 1: yamlcmp reads the BOM skill 'no-frontmatter'" 'has_line "$FX/run1.block" "yamlcmp $I_BOM no-frontmatter"'
check "run 1: both typedprobe builds read the BOM skill 'err'" 'has_line "$FX/run1.block" "tp-skill $I_BOM err"'
check "run 1: on the base's struct yamlcmp reads the duplicated key 'parse-error-both'" 'has_line "$FX/run1.block" "yamlcmp $I_DUP parse-error-both"'
check "run 1: on the base's struct yamlcmp reads the tagged unknown key 'parse-error-old'" 'has_line "$FX/run1.block" "yamlcmp $I_TAG parse-error-old"'
check "run 1: typedprobe-base refuses both, the older wave loader takes both" 'has_line "$FX/run1.block" "tp-skill $I_DUP base=err wave=ok" && has_line "$FX/run1.block" "tp-skill $I_TAG base=err wave=ok"'
check "run 1: the plain, the linked and the UTF-8-named skill show no row (equal, ok)" '! grep -q -E "^(yamlcmp|yamlcmp-value|tp-skill) ($I_GOOD|$I_LINK|$I_UNI) " "$FX/run1.block"'
check "run 1: tomlcmp reads the malformed TOML 'parse-error-both'" 'has_line "$FX/run1.block" "tomlcmp $I_BROKEN parse-error-both"'
check "run 1: tedit reads it 'parse-error-both'" 'has_line "$FX/run1.block" "tedit $I_BROKEN parse-error-both"'
check "run 1: tedit's round trip reads it 'not-applicable'" 'has_line "$FX/run1.block" "tedit-roundtrip $I_BROKEN not-applicable"'
check "run 1: the tomlcmp summary counts one parse error" '[ "$(summary_value "$FX/run1.block" tomlcmp-summary parse-error-both)" = 1 ]'
check "run 1: keycount counts the llm.toml's secret_ref (>0)" '[ "$(summary_value "$FX/run1.block" keycount-llm-summary secret_ref)" -gt 0 ]'
check "run 1: keycount counts use_keychain = true (1)" '[ "$(summary_value "$FX/run1.block" keycount-llm-summary use_keychain-true)" = 1 ]'
check "run 1: keycount counts the plugin config's inline secret_ref (>0)" '[ "$(summary_value "$FX/run1.block" keycount-plugins-summary secret_ref)" -gt 0 ]'
check "run 1: keycount counted both plugin files" '[ "$(summary_value "$FX/run1.block" keycount-plugins-summary counted)" = 2 ]'
check "run 1: the waiver does not hold" 'has_match "$FX/run1.block" '\''^P12-keychain-waiver does-not-hold \(keychain-reference-counted\)$'\'''
check "run 1: every typedprobe kind ran under both builds" '[ "$(grep -c -E '\''^tp-[a-z-]+ inputs=[1-9][0-9]* base\(ok=[0-9]+ err=[0-9]+ panic=[0-9]+\) wave\(ok=[0-9]+ err=[0-9]+ panic=[0-9]+\) disagree=[0-9]+$'\'' "$FX/run1.block")" = 11 ]'
check "run 1: the comm checks moved no version" 'has_match "$FX/run1.block" '\''^typedprobe-base-lock moved=0 '\'' && has_match "$FX/run1.block" '\''^typedprobe-wave-lock moved=0 '\'''
check "run 1: the tools wrote nothing on stderr" 'has_line "$FX/run1.block" "tool-stderr lines=0 panic-hook=0 other=0"'
fixture_state >"$FX/fixture.after"
check "run 1: the fixture is byte-unchanged (independent check)" 'cmp -s "$FX/fixture.before" "$FX/fixture.after"'

# ---- run 2: an empty home, every child given the chance to reach the terminal -----
# Each shim logs its name, writes to fds 3 and 4 (the terminal's, in the
# script) and execs the real command; a wrapper that rustc must never run;
# MAKEFLAGS naming fds 3 and 4 as a jobserver; GIT_TRACE on fd 3.
SH=$FX/shims
mkdir -p "$SH"
for c in rustup cargo git find shasum stat cmp tar perl; do
  real=$(command -v "$c")
  printf '#!/bin/sh\necho %s >>"%s"\n{ echo "SHIM-FD3-%s" >&3; } 2>/dev/null\n{ echo "SHIM-FD4-%s" >&4; } 2>/dev/null\nexec "%s" "$@"\n' \
    "$c" "$FX/shim.log" "$c" "$c" "$real" >"$SH/$c"
  chmod +x "$SH/$c"
done
printf '#!/bin/sh\necho ran >>"%s"\nexec "$@"\n' "$FX/wrapper-ran-SENTINELPATH" >"$FX/wrapper.sh"
chmod +x "$FX/wrapper.sh"
t0=$SECONDS
rc=0
env PATH="$SH:$PATH" RUSTC_WRAPPER="$FX/wrapper.sh" RUSTC_WORKSPACE_WRAPPER="$FX/wrapper.sh" \
  CARGO_BUILD_RUSTC_WRAPPER="$FX/wrapper.sh" MAKEFLAGS='-j4 --jobserver-auth=3,4' GIT_TRACE=3 GIT_TRACE_SETUP=4 \
  /bin/bash "$SCRIPT" --home "$EMPTY" --keep >"$FX/run2.out" 2>"$FX/run2.err" || rc=$?
echo "run 2 took $((SECONDS - t0)) s, exit $rc"
block_of "$FX/run2.out" >"$FX/run2.block"
X2=$(scratch_of "$FX/run2.err")
check "run 2: exit 0" '[ "$rc" = 0 ]'
check "run 2: stderr is the one line naming the scratch dir (no child wrote to fd 4)" 'stderr_is_trailer "$FX/run2.err" && ! grep -q -e SHIM- -e "trace:" "$FX/run2.err"'
check "run 2: stdout is the block alone (no child wrote to fd 3)" 'stdout_is_block "$FX/run2.out" && ! grep -q -e SHIM- -e "trace:" "$FX/run2.out"'
shims_ran() { local c; for c in rustup cargo git find shasum stat cmp tar perl; do grep -q -x "$c" "$FX/shim.log" || return 1; done; }
check "run 2: every shim ran (rustup cargo git find shasum stat cmp tar perl)" 'shims_ran'
check "run 2: the rustc wrapper never ran" '[ ! -e "$FX/wrapper-ran-SENTINELPATH" ]'
check "run 2: stdout and stderr hold no sentinel and no fixture path" 'no_leak "$FX/run2.out" "$FX/run2.err"'
check "run 2: the block keeps to its alphabet" 'alphabet_ok "$FX/run2.block"'
check "run 2: status complete" 'has_line "$FX/run2.block" "status complete"'
check "run 2: every list has zero inputs from the home" '[ "$(grep -c -E '\''^list [a-z-]+ inputs=[0-9]+ home=0 '\'' "$FX/run2.block")" = 13 ]'
check "run 2: keycount had nothing to count" 'has_line "$FX/run2.block" "keycount-llm inputs=0 (not run)" && has_line "$FX/run2.block" "keycount-plugins inputs=0 (not run)"'
check "run 2: the persona probes had no input" 'has_line "$FX/run2.block" "tp-soul inputs=0 (not run)" && has_line "$FX/run2.block" "tp-bootstrap inputs=0 (not run)"'
check "run 2: the waiver does not hold for want of an llm.toml" 'has_line "$FX/run2.block" "P12-keychain-waiver does-not-hold (no-llm-input)"'
check "run 2: store-unchanged yes, inputs-unchanged yes" 'has_line "$FX/run2.block" "store-unchanged yes" && has_line "$FX/run2.block" "inputs-unchanged yes"'
check "run 2: the empty home is still empty" '[ -z "$(ls -A "$EMPTY")" ]'
check "run 2: --keep kept the build tree" '[ -n "$X2" ] && [ -d "$X2/build/target" ] && [ -d "$X2/build/base" ]'
check "run 2: keep-build=yes in the block" 'has_line "$FX/run2.block" "options include-checkout-config=no keep-build=yes tool-time-limit=300"'
# The kept build tree is about 3.5 GB: removed here, the rest kept.
case "$X2" in */consent-point.*) rm -rf -- "$X2/build" ;; esac

# ---- run 3: pathological inputs, a FIFO the DU-10 count reads, a change mid-run ----
# A TOML of 300,000 keys (toml_edit's key removal grows with the square of the
# count) and a SKILL.md nested 200,000 deep (libyaml's simple keys likewise),
# under a 10-second limit per tool run; a zero-count llm.toml and plugin
# config beside a FIFO at plugins/.permissions.toml; a TOML linked in from
# outside the store. While the first tool builds, a watcher sets an extended
# attribute — metadata only, the bytes, mode and mtime unchanged — on that
# outside file (only inputs-unchanged can see it) and on a store file no tool
# reads (only store-unchanged can see it): each line must read `no`, which it
# does only when its snapshot was taken before the builds and records the
# ctime.
H3=$FX/home3-SENTINELPATH
OUT3=$FX/outside3-SENTINELPATH
T3=$SCR/t3
mkdir -p "$H3/config/tools" "$H3/config/skills/a" "$H3/config/skills/deep-SENTINELPATH" "$H3/plugins/.config" "$H3/plugins/.data/p" "$OUT3" "$T3"
printf '[providers.ollama]\nenabled = false\nbase_url = "http://127.0.0.1:1/v1"\n' >"$H3/config/llm.toml"
awk 'BEGIN { for (i = 0; i < 300000; i++) printf "SENTINELKEY%d = \"SENTINELVALUE-%d\"\n", i, i }' >"$H3/config/tools/big-SENTINELPATH.toml"
printf 'linked = "SENTINELVALUE"\n' >"$OUT3/linked-SENTINELPATH.toml"
ln -s "$OUT3/linked-SENTINELPATH.toml" "$H3/config/tools/linked.toml"
printf -- '---\nname: a\ndescription: SENTINELVALUE\n---\nBody\n' >"$H3/config/skills/a/SKILL.md"
{
  printf -- '---\nname: deep\ndescription: d\nSENTINELKEY: '
  head -c 200000 /dev/zero | tr '\0' '['
  printf 'SENTINELVALUE'
  head -c 200000 /dev/zero | tr '\0' ']'
  printf '\n---\nBody\n'
} >"$H3/config/skills/deep-SENTINELPATH/SKILL.md"
printf 'plain = "SENTINELVALUE"\n' >"$H3/plugins/.config/p.toml"
mkfifo "$H3/plugins/.permissions.toml"
printf '{"SENTINELKEY":"SENTINELVALUE"}\n' >"$H3/plugins/.data/p/state.json"
meta3() { stat -f '%Lp %m %z' "$OUT3/linked-SENTINELPATH.toml" "$H3/plugins/.data/p/state.json"; shasum -a 256 "$OUT3/linked-SENTINELPATH.toml" "$H3/plugins/.data/p/state.json"; }
meta3 >"$FX/run3.meta.before"
(
  for _ in $(seq 1 3000); do
    f=$(find "$T3" -mindepth 3 -maxdepth 3 -path '*/logs/build-tomlcmp.log' -size +0 2>/dev/null | head -n 1 || true)
    if [ -n "$f" ]; then
      xattr -w com.example.e6-test during-build "$OUT3/linked-SENTINELPATH.toml"
      xattr -w com.example.e6-test during-build "$H3/plugins/.data/p/state.json"
      # Still building: no tool output exists yet.
      if [ -z "$(ls -A "${f%/logs/build-tomlcmp.log}/run")" ]; then echo injected-during-build; else echo injected-late; fi >"$FX/run3.injected"
      exit 0
    fi
    sleep 0.1
  done
  echo never >"$FX/run3.injected"
) &
WATCH=$!
t0=$SECONDS
rc=0
env TMPDIR="$T3" /bin/bash "$SCRIPT" --home "$H3" --tool-time-limit 10 >"$FX/run3.out" 2>"$FX/run3.err" || rc=$?
echo "run 3 took $((SECONDS - t0)) s, exit $rc"
wait "$WATCH" || true
block_of "$FX/run3.out" >"$FX/run3.block"
X3=$(scratch_of "$FX/run3.err")
meta3 >"$FX/run3.meta.after"
check "run 3: the watcher set its attributes during the builds" '[ "$(cat "$FX/run3.injected")" = injected-during-build ]'
check "run 3: the two files' bytes, mode and mtime are unchanged (the change is metadata only)" 'cmp -s "$FX/run3.meta.before" "$FX/run3.meta.after"'
check "run 3: exit 1 (steps timed out), the block printed" '[ "$rc" = 1 ] && has_line "$FX/run3.block" "exit 1" && has_match "$FX/run3.block" "^status incomplete steps-failed=[0-9]+$"'
check "run 3: tedit record stopped at the limit" 'has_line "$FX/run3.block" "step-failed run-tedit-record-timeout"'
check "run 3: yamlcmp stopped at the limit" 'has_line "$FX/run3.block" "step-failed run-yamlcmp-timeout"'
check "run 3: the limit is in the block" 'has_line "$FX/run3.block" "options include-checkout-config=no keep-build=no tool-time-limit=10"'
check "run 3: the attribute on the fed file outside the store, through a link, during the builds: inputs-unchanged no" 'has_line "$FX/run3.block" "inputs-unchanged no"'
check "run 3: the attribute on a store file during the builds: store-unchanged no" 'has_line "$FX/run3.block" "store-unchanged no"'
check "run 3: the FIFO at plugins/.permissions.toml is skipped" 'has_line "$FX/run3.block" "list plugcfg inputs=1 home=1 extra=0 checkout=0 checkout-local=0 missing=0 skipped=1"'
check "run 3: so the waiver does not hold (skipped-input), though both counts are zero" 'has_line "$FX/run3.block" "P12-keychain-waiver does-not-hold (skipped-input)" && [ "$(summary_value "$FX/run3.block" keycount-plugins-summary secret_ref)" = 0 ] && [ "$(summary_value "$FX/run3.block" keycount-llm-summary secret_ref)" = 0 ]'
check "run 3: stdout is the block alone, stderr the one line naming the scratch dir" 'stdout_is_block "$FX/run3.out" && stderr_is_trailer "$FX/run3.err"'
check "run 3: stdout and stderr hold no sentinel and no fixture path" 'no_leak "$FX/run3.out" "$FX/run3.err"'
check "run 3: the block keeps to its alphabet" 'alphabet_ok "$FX/run3.block"'
check "run 3: the build tree was removed, no process of the run is left" '[ -n "$X3" ] && [ ! -e "$X3/build" ] && no_process_of "$X3"'

# ---- run 4: Ctrl-C during the builds -------------------------------------------------
# SIGINT to the run's process group (what the terminal sends) while cargo
# builds: the block still printed, naming the step, exit 130; the build tree
# removed; no process left; the fixture untouched.
T4=$SCR/t4
mkdir -p "$T4"
set -m
env TMPDIR="$T4" /bin/bash "$SCRIPT" --home "$H" >"$FX/run4.out" 2>"$FX/run4.err" &
P4=$!
set +m
for _ in $(seq 1 1200); do
  ls "$T4"/consent-point.*/logs/build-tomlcmp.log >/dev/null 2>&1 && break
  sleep 0.1
done
sleep 2
kill -INT -- "-$P4" 2>/dev/null || true
rc=0
wait "$P4" || rc=$?
block_of "$FX/run4.out" >"$FX/run4.block"
X4=$(scratch_of "$FX/run4.err")
sleep 1
check "run 4: exit 130, the block says so" '[ "$rc" = 130 ] && has_line "$FX/run4.block" "exit 130" && has_match "$FX/run4.block" "^status incomplete steps-failed=[0-9]+$"'
check "run 4: the block names the interrupted build step" 'has_match "$FX/run4.block" "^step-failed build-(tomlcmp|yamlcmp|keycount|tedit-old|tedit-new|typedprobe-base|typedprobe-wave)-interrupted$"'
check "run 4: the build tree was removed, no process of the run is left" '[ -n "$X4" ] && [ ! -e "$X4/build" ] && no_process_of "$X4"'
check "run 4: stdout is the block alone, stderr the one line naming the scratch dir" 'stdout_is_block "$FX/run4.out" && stderr_is_trailer "$FX/run4.err"'
check "run 4: stdout and stderr hold no sentinel and no fixture path" 'no_leak "$FX/run4.out" "$FX/run4.err"'
check "run 4: the block keeps to its alphabet" 'alphabet_ok "$FX/run4.block"'
fixture_state >"$FX/fixture.after4"
check "run 4: the fixture is byte-unchanged (independent check)" 'cmp -s "$FX/fixture.before" "$FX/fixture.after4"'

CB_AFTER=$({ find "${CARGO_HOME:-$HOME/.cargo}/build" -mindepth 1 -maxdepth 1 2>/dev/null || true; } | wc -l | tr -d ' ')
check "no build left intermediates under \$CARGO_HOME/build (MF-5)" '[ "$CB_BEFORE" = "$CB_AFTER" ]'

echo
for n in 1 2 3 4; do
  echo "--- run $n's output ---"
  cat "$FX/run$n.out"
done
echo
echo "$PASS passed, $FAILS failed (fixture and outputs: $FX; scratch dirs of runs 3 and 4: $SCR)"
[ "$FAILS" = 0 ]
