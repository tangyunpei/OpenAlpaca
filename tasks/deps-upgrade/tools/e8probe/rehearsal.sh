#!/bin/bash
set +o xtrace +o verbose   # an inherited xtrace or verbose (SHELLOPTS, BASH_ENV) is off before any line holding an argument runs
# rehearsal.sh — programme-plan §4.5 E8's steps 2–8 as one owner-run command:
# the read-only rehearsal on a COPY of the owner's store, with no daemon, by
# the binaries the lead hands over (README.md here, *One command*).
#
#   /bin/bash tasks/deps-upgrade/tools/e8probe/rehearsal.sh --copy "$T/realcopy" --bins <the folder handed over>
#   /bin/bash tasks/deps-upgrade/tools/e8probe/rehearsal.sh --help     # prints step 1, for the owner to type
#
# Step 1 stays the owner's own typed commands (`--help` prints them): quit the
# app, stop the real daemon and wait for exit and lock release, `cp -Rp` the
# store into a fresh 0700 directory $T, check that neither the copy nor its
# state/ is a symbolic link (`cp -Rp` copies a link as a link, so a store kept
# behind one would leave the copy pointing at the real files), and only then
# remove the copy's state/discovery.json and state/openalpacad.lock. This
# script never names the real store and never stops or starts a daemon.
#
# When it ends, stdout holds the block and nothing else: paste everything
# between `=== PASTE FROM HERE ===` and `=== TO HERE ===` (`| pbcopy` takes
# exactly that). The block holds only commits and binary hashes, versions,
# fixed words, counts and partition keys — never a file name, a path, a note
# id, a value or a byte of a file; a last filter drops any line with a byte
# outside [A-Za-z0-9 ._:=(),+-] and tab, under LC_ALL=C. One line on stderr
# after the block names the work directory: it holds the doctors' full
# reports, step 3's per-table output, the probe's --out files and the logs,
# and stays under $T beside the copy. It is not for pasting; step 9's
# `rm -rf "$T"` removes it with the copy.
#
# Options
#   --copy <dir>        Required: the owner's `cp -Rp` of the store. Refused,
#                       before anything else is read, when any component of it
#                       (as given or resolved, in any letter case) is named
#                       `.openalpaca`; when it is itself a symbolic link, or
#                       holds one in its top two levels or in two levels of
#                       its state/ (every step would follow it out of the
#                       copy: the doctors' read-write open, the probe, the
#                       cache copies); and when its state/discovery.json or
#                       state/openalpacad.lock exists (E8 step 2: a doctor
#                       branches on the copy's discovery file, and a pid that
#                       is alive again would send it to the real daemon).
#   --bins <dir>        Required: e8probe-old, e8probe-new, openalpaca-prewave,
#                       openalpaca-tip and SHA256SUMS (`shasum -a 256` form)
#                       listing each of those four once; another entry (E7's
#                       openalpacad-prewave) may be there and is checked too.
#                       Checked first: nothing of the copy is opened when a
#                       sum fails.
#   --tool-time-limit <seconds>
#                       The wall-clock limit of each tool run; default 900. A
#                       run past it is killed and fails as `<step>-timeout`.
#
# What it does, in order (the plan's step numbers)
#   pre   The refusals above; --bins complete and its sums good; perl present.
#   2     A fresh 0700 work dir beside the copy (under $T); every child run
#         with `env -i`: PATH=/usr/bin:/bin:/usr/sbin:/sbin, HOME and TMPDIR in
#         the work dir, OPENALPACA_HOME_STORE=<copy>,
#         OPENALPACA_CONFIG_DIR=<copy>/config, NO_COLOR=1, LC_ALL=C, from a
#         scratch working directory, its stdin /dev/null and its streams to
#         files; `real-daemon-running` (pgrep, informational).
#   2b    The cache copies for the probe, taken before anything opens the copy:
#         state/cache/memory-index-v1.sqlite* into cache-ro, cache-rw-old,
#         cache-rw-new.
#   3     A raw read-only connection first: the probe's `db` check (mode=ro),
#         then E8 step 3's sqlite3 block verbatim, saved as out/step3.txt. The
#         probe goes first because the system sqlite3 cannot open a WAL
#         database read-only when it has no -wal and -shm — a cleanly stopped
#         daemon's store has neither — while the probe's SQLite creates the
#         two sidecars (a zero-length -wal) and reads it (README, *Proving
#         read-only*).
#   4     The independent inventory (R10): `e8probe-new inventory`.
#   5–6   `openalpaca-prewave memory doctor --global`, then the tip's, no
#         --repair, stdout and stderr apart: the first line must read
#         `No daemon is running; scanning locally.` (else the report came over
#         HTTP and the rehearsal is void); the partition keys and health words
#         compared, and against the inventory.
#   7–8   `e8probe-old all`, `e8probe-new all`, `e8probe-old all` (the search
#         recorded once, compared twice); the runs compared line by line, the
#         SQLite version lines aside.
#   then  `copy-memory-unchanged`: every entry under the copy's memory/,
#         projects/ and state/memory-control/ with its type, mode, size,
#         mtime, ctime and inode, before step 3 and after step 8 (neither
#         doctor nor the probe writes there); a `verdict` line reading the
#         plan's gate from the lines above (pass, fail with its reasons, or
#         void); the block.
# Exit 0 when every step ran (a difference is a result, read in the block);
# 1 when a step could not run (`step-failed <step>`); 2 for a usage error or a
# refusal, before anything is read; 130, 143 and 129 after Ctrl-C, TERM and a
# hangup, the block still printed. bash 3.2 (/bin/bash on macOS) or later.

# Under sh or another shell, run again under /bin/bash with nothing of the
# caller's start-up carried over.
if [ -z "${BASH_VERSION:-}" ] || (shopt -oq posix) 2>/dev/null; then
  if [ -n "${E8_REHEARSAL_REEXEC:-}" ]; then
    echo "rehearsal: run it with /bin/bash" >&2
    exit 2
  fi
  exec env -u POSIXLY_CORRECT -u BASH_ENV -u ENV -u SHELLOPTS -u BASHOPTS -u BASH_XTRACEFD -u CDPATH \
    E8_REHEARSAL_REEXEC=1 /bin/bash "$0" "$@"
fi

set -euo pipefail
trap - DEBUG ERR RETURN 2>/dev/null || true
export LC_ALL=C
unset CDPATH GIT_DIR GIT_WORK_TREE E8_SELFTEST_PANIC

PROG=rehearsal
TAB=$(printf '\t')
DOCTOR_FIRST='No daemon is running; scanning locally.'
BIN_NAMES="e8probe-old e8probe-new openalpaca-prewave openalpaca-tip"

die_usage() {
  # A fixed text only: an argument could be a path.
  echo "$PROG: $1" >&2 || true
  echo "usage: $PROG --copy <the cp -Rp of the store> --bins <the folder handed over> [--tool-time-limit <seconds>] | --help" >&2 || true
  exit 2
}

print_help() {
  cat <<'EOF'
E8 — a rehearsal on a copy of your store, read-only, no daemon (programme-plan §4.5 E8).

Step 1 is yours, typed in bash (the script never names the real store):

  # Quit the app. Stop the real daemon and wait for exit AND lock release:
  openalpaca daemon stop
  pgrep -x openalpacad && echo 'STILL RUNNING - stop here'
  T=$(realpath "$(mktemp -d)"); chmod 700 "$T"
  cp -Rp ~/.openalpaca "$T/realcopy"     # holds .master_key and encrypted keys: it stays under $T
  [ -L "$T/realcopy" ] && echo 'THE COPY IS A LINK - stop here'
  [ -L "$T/realcopy/state" ] && echo 'ITS state IS A LINK - stop here'
  rm -f "$T/realcopy/state/discovery.json" "$T/realcopy/state/openalpacad.lock"
  # the real daemon stays stopped until the rehearsal is done

Steps 2 to 8, one command, from the checkout's root:

  /bin/bash tasks/deps-upgrade/tools/e8probe/rehearsal.sh --copy "$T/realcopy" --bins <the folder the lead handed over>

Paste only the block between === PASTE FROM HERE === and === TO HERE ===.

Step 9: rm -rf "$T", and only then restart the real daemon if wanted. Or keep the copy as
§5 item 4's backup: moved to a 0700 directory outside any repository, its checksum list
recorded, provided no daemon has started on the real store since step 1.
EOF
  exit 0
}

COPY_ARG=""
BINS_ARG=""
LIMIT=900
while [ $# -gt 0 ]; do
  case "$1" in
    --copy) [ $# -ge 2 ] || die_usage "--copy needs a directory"; COPY_ARG=$2; shift 2 ;;
    --copy=*) COPY_ARG=${1#--copy=}; shift ;;
    --bins) [ $# -ge 2 ] || die_usage "--bins needs a directory"; BINS_ARG=$2; shift 2 ;;
    --bins=*) BINS_ARG=${1#--bins=}; shift ;;
    --tool-time-limit) [ $# -ge 2 ] || die_usage "--tool-time-limit needs a number of seconds"; LIMIT=$2; shift 2 ;;
    --tool-time-limit=*) LIMIT=${1#--tool-time-limit=}; shift ;;
    -h | --help) print_help ;;
    *) die_usage "unknown argument" ;;
  esac
done
[ -n "$COPY_ARG" ] || die_usage "--copy is required"

# ---- pre: the refusals, before anything else ---------------------------------------
# A `.openalpaca` component in any letter case, as given, then resolved; a
# copy that is a link or holds one where a step would follow it; the copy's
# discovery file and daemon lock, as entries (a dangling link counts).
has_dot_store() {   # the default APFS volume is case-insensitive: .OpenAlpaca is the same folder
  case "/$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')/" in */.openalpaca/*) return 0 ;; esac
  return 1
}
canon() {   # /bin/pwd -P is getcwd(3), in the disk's letter case; bash's builtin keeps the case typed
  local p=$1
  case "$p" in -*) p=./$p ;; esac
  (CDPATH='' cd -P -- "$p" >/dev/null 2>&1 && /bin/pwd -P)
}
has_nl() { case "$1" in *$'\n'*) return 0 ;; esac; return 1; }
strip_tail() {   # the path without trailing `/` or `/.`, so that -L tests its last component itself
  local s=$1
  while :; do
    case "$s" in
      / | .) break ;;
      */) s=${s%/} ;;
      */.) s=${s%/.} ;;
      *) break ;;
    esac
  done
  printf '%s' "$s"
}
links_in_copy() {   # the symbolic links where a step would follow one: the copy's top two levels, two levels of state/
  local a b=0
  a=$(find "$COPY" -mindepth 1 -maxdepth 2 -type l -print 2>/dev/null | wc -l) || return 1
  if [ -d "$COPY/state" ] && [ ! -L "$COPY/state" ]; then
    b=$(find "$COPY/state" -mindepth 2 -maxdepth 2 -type l -print 2>/dev/null | wc -l) || return 1   # state/*/*: state/* is the first find's
  fi
  echo $((a + b))
}
! has_dot_store "$COPY_ARG" || die_usage "refused: --copy has a component named .openalpaca; pass the copy under \$T, never the store itself"
COPY=$(canon "$COPY_ARG") || die_usage "--copy is not a directory that can be entered"
! has_nl "$COPY" || die_usage "--copy's path holds a line break"
! has_dot_store "$COPY" || die_usage "refused: --copy resolves through a component named .openalpaca; pass the copy under \$T, never the store itself"
! [ -L "$(strip_tail "$COPY_ARG")" ] || die_usage "refused: --copy is a symbolic link (cp -Rp copied a link to the store, not the store); copy the folder it names instead"
NLINKS=$(links_in_copy) || die_usage "refused: the copy cannot be listed for symbolic links"
[ "$NLINKS" = 0 ] || die_usage "refused: the copy holds $NLINKS symbolic link(s) in its top two levels or two levels of state/, which a step would follow out of the copy; list them with: find \"\$T/realcopy\" -maxdepth 3 -type l; remove each from the copy (rm the link itself, no trailing slash, never what it names) or put a copy of what it names in its place, and run again"
for f in state/discovery.json state/openalpacad.lock; do
  if [ -e "$COPY/$f" ] || [ -L "$COPY/$f" ]; then
    die_usage "refused: the copy holds $f; remove it from the copy (rehearsal.sh --help, step 1) and run again"
  fi
done
[ -f "$COPY/state/openalpaca.db" ] && [ ! -L "$COPY/state/openalpaca.db" ] || die_usage "--copy has no state/openalpaca.db"
[ -n "$BINS_ARG" ] || die_usage "--bins is required"
BINS=$(canon "$BINS_ARG") || die_usage "--bins is not a directory that can be entered"
! has_nl "$BINS" || die_usage "--bins's path holds a line break"
case "$LIMIT" in "" | *[!0-9]* | 0*) die_usage "--tool-time-limit takes a whole number of seconds, 1 to 86400" ;; esac
[ "${#LIMIT}" -le 5 ] && [ "$LIMIT" -le 86400 ] || die_usage "--tool-time-limit takes a whole number of seconds, 1 to 86400"
for b in $BIN_NAMES; do
  [ -f "$BINS/$b" ] && [ -x "$BINS/$b" ] || die_usage "--bins lacks an executable $b"
done
[ -f "$BINS/SHA256SUMS" ] || die_usage "--bins lacks SHA256SUMS"
command -v perl >/dev/null 2>&1 || die_usage "perl must be on PATH (it bounds each tool's run)"
[ -x /usr/bin/sqlite3 ] || die_usage "/usr/bin/sqlite3 is needed for step 3"
if ! [ -e /dev/fd/1 ] || ! [ -e /dev/fd/2 ]; then die_usage "stdout or stderr is closed"; fi

# ---- 2. the work dir, beside the copy (under $T) ---------------------------------------
PARENT=$(dirname -- "$COPY")
umask 077
W_MADE=$(mktemp -d "$PARENT/e8-rehearsal.XXXXXX" 2>/dev/null) || die_usage "mktemp failed beside the copy"
# shellcheck disable=SC2329  # invoked by the EXIT trap
early_exit() {
  case "$W_MADE" in "$PARENT"/e8-rehearsal.?*) rm -rf -- "$W_MADE" 2>/dev/null || true ;; esac
}
trap early_exit EXIT
W=$(canon "$W_MADE") || die_usage "the work dir cannot be entered"
LOGS=$W/logs
OUT=$W/out
RUN=$W/run
mkdir -p "$LOGS" "$OUT" "$RUN" "$W/home" "$W/tmp" "$W/cwd" "$W/cache-ro" "$W/cache-rw-old" "$W/cache-rw-new" 2>/dev/null ||
  die_usage "the work dir cannot be written"
BODY=$W/paste.body

# fd 3 is the terminal's stdout, for the block alone; fd 4 its stderr, for the
# line after the block. No child gets either.
exec 3>&1 4>&2
exec 1>>"$LOGS/script.stdout" 2>>"$LOGS/script.stderr" </dev/null

FAILED=""
STEP=start
BLOCK_DONE=0
START_SECONDS=$SECONDS

emit() { printf '%s\n' "$*" >>"$BODY"; }
fail() {
  case " $FAILED " in *" $1 "*) return 0 ;; esac
  FAILED="$FAILED $1"
  emit "step-failed $1"
}
fail_rc() {   # step rc
  case "$2" in
    142) fail "$1-timeout" ;;
    129 | 130 | 143) fail "$1-interrupted" ;;
    *) fail "$1" ;;
  esac
}

# The block's alphabet, byte by byte (LC_ALL=C): a line holding any byte
# outside [A-Za-z0-9 ._:=(),+-] and tab — '/' among them, so no path — is
# dropped and counted, never printed.
block_filter() {
  local LC_ALL=C line dropped=0
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      *[!A-Za-z0-9\ ._:=\(\),+"$TAB"-]*) dropped=$((dropped + 1)) ;;
      *) printf '%s\n' "$line" ;;
    esac
  done
  [ "$dropped" = 0 ] || echo "dropped-lines $dropped (outside the block's alphabet; nothing of them printed)"
}

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
print_trailer() {
  echo "($PROG, not for pasting: $W holds the doctors' full reports, step 3's output, the probe's detail files and the logs; it lies beside the copy and goes with it.)" >&4 || true
}
finish() {   # rc
  trap '' INT TERM HUP
  print_block "$1"
  print_trailer
  exit "$1"
}
# shellcheck disable=SC2329  # invoked by the EXIT trap
on_exit() {
  local rc=$?
  trap '' INT TERM HUP
  if [ "$BLOCK_DONE" = 0 ]; then
    [ "$rc" != 0 ] || rc=1
    case "$rc" in
      129 | 130 | 143) fail "$STEP-interrupted" ;;
      *) fail "$STEP-unexpected" ;;
    esac
    print_block "$rc"
    print_trailer
  fi
  exit "$rc"
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

# Run one tool from the scratch working directory under `env -i` and the time
# limit (perl sets the alarm and execs the tool in its place), its streams to
# files.
# shellcheck disable=SC2016  # perl code, expanded by perl
LIMIT_PL='$SIG{ALRM} = "DEFAULT"; alarm shift @ARGV; exec { $ARGV[0] } @ARGV or exit 127'
run() {   # step out err cmd...
  local step=$1 out=$2 err=$3 rc=0
  shift 3
  STEP=$step
  (cd "$W/cwd" && exec env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin HOME="$W/home" TMPDIR="$W/tmp" LC_ALL=C NO_COLOR=1 \
    OPENALPACA_HOME_STORE="$COPY" OPENALPACA_CONFIG_DIR="$COPY/config" \
    perl -e "$LIMIT_PL" "$LIMIT" "$@") >"$out" 2>"$err" </dev/null 3>&- 4>&- || rc=$?
  return "$rc"
}
lines() { wc -l <"$1" | tr -d ' '; }

emit "rehearsal v1"
# The commit this script was taken from, when it runs from a checkout (git
# reads objects only; GIT_OPTIONAL_LOCKS=0 keeps it from writing the index).
SCRIPT_DIR=$(canon "$(dirname -- "$0")") || SCRIPT_DIR=""
SCRIPT_SHA=$(GIT_OPTIONAL_LOCKS=0 git -C "$SCRIPT_DIR" rev-parse --verify --quiet HEAD 2>/dev/null </dev/null 3>&- 4>&-) || SCRIPT_SHA=""
case "$SCRIPT_SHA" in
  *[!0-9a-f]* | "") emit "script commit unknown" ;;
  *) if GIT_OPTIONAL_LOCKS=0 git -C "$SCRIPT_DIR" diff --quiet HEAD -- "$SCRIPT_DIR" 2>/dev/null </dev/null 3>&- 4>&-; then
       emit "script commit $SCRIPT_SHA tools-dir-modified no"
     else
       emit "script commit $SCRIPT_SHA tools-dir-modified yes"
     fi ;;
esac
emit "options tool-time-limit=$LIMIT"

# ---- 2. the binaries --------------------------------------------------------------------
STEP=bins
# SHA256SUMS must list each of the four names once, and every sum it lists
# hold (`shasum -c` also fails on a listed file that is missing).
SUMS_OK=1
if ! awk -v want="$BIN_NAMES" '
    BEGIN { n = split(want, w, " "); for (i = 1; i <= n; i++) need[w[i]] = 1 }
    NF != 2 || length($1) != 64 || $1 ~ /[^0-9a-f]/ { bad = 1; next }
    { name = $2; sub(/^\*/, "", name); if (name in seen) bad = 1; seen[name] = 1 }
    END { for (k in need) if (!(k in seen)) bad = 1; exit bad }' "$BINS/SHA256SUMS" 2>>"$LOGS/bins.log"; then
  SUMS_OK=0
fi
if [ "$SUMS_OK" = 1 ] && ! (cd "$BINS" && shasum -a 256 -c SHA256SUMS) >>"$LOGS/bins.log" 2>&1 </dev/null 3>&- 4>&-; then
  SUMS_OK=0
fi
if [ "$SUMS_OK" = 0 ]; then
  emit "bins-checksums failed"
  fail bins-checksums
  finish 1
fi
emit "bins-checksums ok"
for b in $BIN_NAMES; do
  emit "bin $b sha256 $(shasum -a 256 "$BINS/$b" 3>&- 4>&- | cut -d' ' -f1)"
done
for v in old new; do
  if run "version-$v" "$RUN/version-$v.out" "$RUN/version-$v.err" "$BINS/e8probe-$v" version; then
    while IFS= read -r l; do emit "e8probe-$v $l"; done <"$RUN/version-$v.out"
  else
    fail_rc "version-$v" "$?"
  fi
done
if pgrep -x openalpacad >/dev/null 2>&1 </dev/null 3>&- 4>&-; then emit "real-daemon-running yes"; else emit "real-daemon-running no"; fi

# The copy's memory, before anything opens it: every entry under memory/,
# projects/ and state/memory-control/ with its type, mode, size, mtime, ctime
# and inode (any write moves the ctime; a read never does).
copy_state() {
  local d
  for d in memory projects state/memory-control; do
    if [ -e "$COPY/$d" ] || [ -L "$COPY/$d" ]; then
      echo "present $d"
      (cd "$COPY" && { find "$d" -exec stat -f '%N %HT %Lp %z %m %c %i' {} + 2>&1 || true; } | LC_ALL=C sort)
    else
      echo "absent $d"
    fi
  done
}
STEP=copy-state-before
copy_state >"$W/copy-state.before" 2>>"$LOGS/copy-state.log" 3>&- 4>&- || fail copy-state-before

# ---- 2b. the cache copies, before anything opens the copy --------------------------------
STEP=cache-copies
IDX=$COPY/state/cache/memory-index-v1.sqlite
if [ -f "$IDX" ] && [ ! -L "$IDX" ]; then
  n=0
  for d in cache-ro cache-rw-old cache-rw-new; do
    for f in "$IDX" "$IDX-wal" "$IDX-shm"; do
      if [ -f "$f" ]; then cp -p "$f" "$W/$d/" 3>&- 4>&- || fail cache-copies; n=$((n + 1)); fi
    done
  done
  emit "cache-copies files=$n"
else
  emit "cache-copies index-absent"
fi

# ---- 3. the raw read-only connection -------------------------------------------------------
STEP=step3
uri_path() {   # percent-encode a path for a file: URI, byte by byte
  local s=$1 out="" c i
  for ((i = 0; i < ${#s}; i++)); do
    c=${s:i:1}
    case "$c" in
      [A-Za-z0-9/._~-]) out=$out$c ;;
      *) out=$out$(printf '%%%02X' $(($(printf '%d' "'$c") & 255))) ;;   # masked: bash 3.2 sign-extends a byte >= 0x80
    esac
  done
  printf '%s' "$out"
}
if run step3-probe "$OUT/step3-probe.stdout" "$RUN/step3-probe.err" \
  "$BINS/e8probe-new" db --db "$COPY/state/openalpaca.db" --out "$OUT/step3-probe.txt"; then
  while IFS= read -r l; do emit "step3-probe $l"; done <"$OUT/step3-probe.stdout"
else
  fail_rc step3-probe "$?"
fi
DBURI="file:$(uri_path "$COPY/state/openalpaca.db")?mode=ro"
# E8 step 3's block, verbatim but for the URI's encoding, -batch, -safe (no
# writefile(), edit() or ATTACH, which mode=ro does not stop) and a table
# name's double quotes doubled; under `env -i` with HOME in the work dir, so
# no ~/.sqliterc is read.
# shellcheck disable=SC2016  # the block's own text, expanded by the inner bash
STEP3_SH='
DB=$1
{ sqlite3 -safe -batch "$DB" "SELECT max(version) FROM schema_version;"
  sqlite3 -safe -batch "$DB" "SELECT type, name, sql FROM sqlite_master ORDER BY type, name;"
  sqlite3 -safe -batch "$DB" "SELECT name FROM sqlite_master WHERE type='"'"'table'"'"' AND name NOT LIKE '"'"'sqlite_%'"'"' ORDER BY name;" |
    while read -r t; do q=${t//\"/\"\"}; printf "%s\t%s\n" "$t" "$(sqlite3 -safe -batch "$DB" "SELECT count(*) FROM \"$q\";")"; done
  sqlite3 -safe -batch "$DB" "SELECT count(*), count(DISTINCT project_root) FROM file_assets WHERE in_project = 1;"
  sqlite3 -safe -batch "$DB" "SELECT count(*) FROM file_assets WHERE status = '"'"'error'"'"';"
} 2>&1
'
if run step3-sqlite "$OUT/step3.txt" "$RUN/step3-sqlite.err" /bin/bash -c "$STEP3_SH" step3 "$DBURI"; then
  first=$(head -n 1 "$OUT/step3.txt")
  case "$first" in "" | *[!0-9]*) first=unreadable ;; esac
  emit "step3 schema-version $first"
  # The per-table lines: `<table>\t<count>`.
  emit "step3 $(awk -F '\t' 'NF == 2 && $2 ~ /^[0-9]+$/ { n++; s += $2 } END { printf "tables=%d rows-total=%d", n, s }' "$OUT/step3.txt")"
  emit "step3 error-lines $(grep -c -E '^(Error|Parse error|Runtime error)' "$OUT/step3.txt" || true)"
else
  fail_rc step3-sqlite "$?"
fi

# ---- 4. the independent inventory (R10) ------------------------------------------------------
if run inventory "$RUN/inventory.out" "$RUN/inventory.err" \
  "$BINS/e8probe-new" inventory --store "$COPY" --out "$OUT/inventory.tsv"; then
  while IFS= read -r l; do emit "$l"; done <"$RUN/inventory.out"
else
  fail_rc inventory "$?"
fi
awk -F '\t' '$1 == "inventory" && $2 == "partition" { print $3 }' "$RUN/inventory.out" | LC_ALL=C sort -u >"$RUN/inventory.keys"

# ---- 5–6. the two doctors ----------------------------------------------------------------------
# A partition line of print_doctor (apps/openalpaca/src/commands/memory.rs):
# `<mark> <scope> <partition key>[  (orphan: …)]  <health>`, its root on the
# next, indented line. Only the key and the health word leave the work dir.
doctor_parts() {   # report -> "<key> <health>" lines, sorted
  sed $'s/\033\\[[0-9;]*m//g' "$1" | awk '
    /^[^ ]/ && ($2 == "global" || $2 == "home" || $2 == "project") &&
      ($3 ~ /^home-[0-9a-f]{32}$/ || $3 ~ /^proj-[0-9a-f]{32}-[0-9a-f]{16}$/) && $NF ~ /^[a-z]+$/ { print $3, $NF }' |
    LC_ALL=C sort
}
DOCTOR_VOID=""
for v in prewave tip; do
  STEP=doctor-$v
  rc=0
  run "doctor-$v" "$OUT/doctor-$v.txt" "$OUT/doctor-$v.err" "$BINS/openalpaca-$v" memory doctor --global || rc=$?
  if [ "$rc" != 0 ]; then fail_rc "doctor-$v" "$rc"; fi
  first=$(sed $'s/\033\\[[0-9;]*m//g' "$OUT/doctor-$v.txt" | head -n 1)
  if [ "$first" = "$DOCTOR_FIRST" ]; then emit "doctor-$v local"; else emit "doctor-$v VOID"; DOCTOR_VOID="$DOCTOR_VOID $v"; fi
  emit "doctor-$v exit $rc stdout-lines $(lines "$OUT/doctor-$v.txt") stderr-lines $(lines "$OUT/doctor-$v.err")"
  doctor_parts "$OUT/doctor-$v.txt" >"$RUN/doctor-$v.parts"
  emit "doctor-$v partitions $(lines "$RUN/doctor-$v.parts")"
  while read -r k h; do emit "doctor-$v partition $k $h"; done <"$RUN/doctor-$v.parts"
  cut -d' ' -f1 "$RUN/doctor-$v.parts" | LC_ALL=C sort -u >"$RUN/doctor-$v.keys"
  emit "doctor-$v-vs-inventory missing=$(LC_ALL=C comm -23 "$RUN/inventory.keys" "$RUN/doctor-$v.keys" | wc -l | tr -d ' ') extra=$(LC_ALL=C comm -13 "$RUN/inventory.keys" "$RUN/doctor-$v.keys" | wc -l | tr -d ' ')"
done
if cmp -s "$OUT/doctor-prewave.txt" "$OUT/doctor-tip.txt"; then emit "doctors-equal yes"; else emit "doctors-equal no"; fi
if cmp -s "$RUN/doctor-prewave.parts" "$RUN/doctor-tip.parts"; then emit "doctor-partitions-equal yes"; else emit "doctor-partitions-equal no"; fi

# ---- 7–8. the probe, old, new, old ---------------------------------------------------------------
n=0
for v in old new old; do
  n=$((n + 1))
  rc=0
  run "probe-$n-$v" "$RUN/probe-$n-$v.out" "$RUN/probe-$n-$v.err" "$BINS/e8probe-$v" all --store "$COPY" \
    --cache-ro "$W/cache-ro/memory-index-v1.sqlite" --cache-rw "$W/cache-rw-$v/memory-index-v1.sqlite" \
    --step3 "$OUT/step3.txt" --record "$OUT/search.record" --out "$OUT/probe-$n-$v.txt" || rc=$?
  # Exit 1 is a check that printed `error` or `panic` (a result, in the
  # lines); anything else is a run that did not finish.
  case "$rc" in 0 | 1) ;; *) fail_rc "probe-$n-$v" "$rc" ;; esac
  emit "p$n-$v exit $rc stderr-lines $(lines "$RUN/probe-$n-$v.err")"
  while IFS= read -r l; do emit "p$n-$v $l"; done <"$RUN/probe-$n-$v.out"
done
words() { grep -v -E "(^|$TAB)(all )?sqlite [0-9.]+\$" "$1" || true; }
nosearch() { words "$1" | grep -v "^search$TAB" || true; }
if [ "$(words "$RUN/probe-2-new.out")" = "$(words "$RUN/probe-3-old.out")" ]; then emit "probe-new-vs-old equal"; else emit "probe-new-vs-old different"; fi
if [ "$(nosearch "$RUN/probe-1-old.out")" = "$(nosearch "$RUN/probe-3-old.out")" ]; then emit "probe-old-vs-old equal"; else emit "probe-old-vs-old different"; fi

STEP=copy-state-after
if copy_state >"$W/copy-state.after" 2>>"$LOGS/copy-state.log" 3>&- 4>&- && cmp -s "$W/copy-state.before" "$W/copy-state.after"; then
  emit "copy-memory-unchanged yes"
else
  emit "copy-memory-unchanged no"
fi

# ---- the verdict: the plan's gate, read from the lines above ---------------------------------------
STEP=verdict
REASONS=""
reason() { REASONS="$REASONS${REASONS:+,}$1"; }
body_has() { grep -q -x -F -- "$1" "$BODY"; }
body_any() { grep -q -E -- "$1" "$BODY"; }
body_has "doctor-partitions-equal yes" || reason doctor-partitions
body_has "probe-new-vs-old equal" || reason probe-new-vs-old
body_has "probe-old-vs-old equal" || reason probe-old-vs-old
body_any "^p[0-9]-(old|new) (db|index)${TAB}integrity not-ok\$" && reason integrity
body_any "^p[0-9]-(old|new) fts-rw${TAB}[a-z_]+ (corrupt|error)\$" && reason fts
body_any "^p[0-9]-(old|new) [a-z-]+${TAB}(error|panic)\$" && reason probe-error
body_any "^p[0-9]-(old|new) db${TAB}step3-counts different\$" && reason step3-counts
body_any "^p[0-9]-(old|new) snapshots${TAB}total$TAB.*${TAB}key-different=[1-9]" && reason key-different
body_any "^p[0-9]-(old|new) snapshots${TAB}total$TAB.*${TAB}(mismatched|missing|duplicate|oversize|unreadable)=[1-9]" && reason note-mismatch
body_any "^p[0-9]-(old|new) search${TAB}queries [0-9]+ with-results [0-9]+ equal [0-9]+ different [1-9]" && reason search-different
body_any "^p[0-9]-(old|new) search${TAB}queries [0-9]+ with-results [0-9]+ equal [0-9]+ different [0-9]+ error [1-9]" && reason search-error
body_any "^p[0-9]-(old|new) search${TAB}queries [0-9]+ with-results 0 " && reason search-vacuous
# No snapshot compared: a store that has seen a daemon has at least the home one.
body_any "^p[0-9]-(old|new) snapshots${TAB}control-dir " && reason snapshots-absent
body_any "^p[0-9]-(old|new) snapshots${TAB}total${TAB}count=0${TAB}" && reason snapshots-vacuous
body_has "copy-memory-unchanged yes" || reason copy-memory
[ -z "$FAILED" ] || reason steps-failed
if [ -n "$DOCTOR_VOID" ]; then
  emit "verdict void (doctor$(echo "$DOCTOR_VOID" | tr ' ' '-'))${REASONS:+ also=$REASONS}"
elif [ -n "$REASONS" ]; then
  emit "verdict fail reasons=$REASONS"
else
  emit "verdict pass"
fi
emit "elapsed seconds=$((SECONDS - START_SECONDS))"

STEP=end
RC=0
[ -z "$FAILED" ] || RC=1
finish "$RC"
