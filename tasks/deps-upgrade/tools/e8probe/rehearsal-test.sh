#!/bin/bash
# rehearsal-test.sh — a re-runnable test of rehearsal.sh on synthetic stores
# only (README.md here, *One command*). It builds corpus B with
# probe-test.sh --corpus-only under a fresh `mktemp -d`, takes its own
# `cp -Rp` of the original as the copy, and runs rehearsal.sh over it with the
# real binaries and with fakes. It never reads or names a real store, starts
# no daemon and reaches no network.
#
#   /bin/bash tasks/deps-upgrade/tools/e8probe/rehearsal-test.sh --bins <dir> [--work <dir>]
#
# --bins is a folder as the lead hands it to the owner: e8probe-old,
# e8probe-new, openalpaca-prewave, openalpaca-tip and SHA256SUMS. The fakes
# (a doctor that answers as a daemon would, one that differs, one that hangs)
# are shell scripts in folders of their own beside copies of the real probes,
# each with its own SHA256SUMS. Exit 0 when every check passes.
#
# The runs:
#   real     the real binaries on the copy, whose path holds MARKER-e8 as its
#            notes, owner id, project folder and rows do: exit 0, `verdict
#            pass`, both doctors local and equal (home quarantined: R09's
#            relocation; the project ready; an earlier build's project
#            snapshot listed apart, its notes `unlocated` in the probe, the
#            inventory's one key the doctors do not report), the probe old = new = old, the
#            copy's memory unchanged (checked by the test too), stdout the
#            block alone with no marker, no `.openalpaca` and nothing dropped,
#            stderr the one line naming the work dir
#   no index the copy without state/cache: `cache-copies index-absent`, the
#            three cache checks `absent`, still `verdict pass`
#   flipped  one note byte changed in the copy: `verdict fail` naming
#            note-mismatch
#   non-ASCII a copy whose path holds a byte >= 0x80: step 3's URI right
#            (schema 4, the same row total), `verdict pass`
#   no snaps the copy without state/memory-control: `verdict fail` naming
#            snapshots-absent and snapshots-vacuous
#   refusals a .openalpaca component (as given, in other letter cases, and
#            through a link, to a folder named in either case), a copy that
#            is itself a link, one whose state/ is a link (its target left
#            untouched: no -wal, no -shm), one holding a link two levels down
#            (config/llm.toml) or two levels under state/ (a cache sidecar), a
#            copy holding state/discovery.json, one holding
#            state/openalpacad.lock (a dangling link there is refused as a
#            link, before the two files are looked at), a missing or
#            incomplete --bins, an unknown argument: exit 2, one fixed line,
#            nothing made beside the copy and no -shm in it; --help prints
#            step 1 (its link checks before the rm) and exits 0
#   sums     a binary changed after SHA256SUMS, and a SHA256SUMS naming
#            another file: exit 1, `bins-checksums failed`, nothing of the copy
#            opened
#   void     a tip doctor without the first line (a report over HTTP):
#            `doctor-tip VOID`, `verdict void`
#   differ   a tip doctor whose health word differs: `doctor-partitions-equal
#            no`, `verdict fail`
#   timeout  a tip doctor that hangs, --tool-time-limit 2:
#            `step-failed doctor-tip-timeout`, exit 1, the block printed
#   TERM     the process group sent TERM while a doctor hangs: exit 143, the
#            block printed, `status incomplete`

# The checks below are shell strings run by eval, so their variables expand
# late (SC2016, and SC2034 for WD, read only inside them).
# shellcheck disable=SC2016,SC2034

set -euo pipefail
export LC_ALL=C
unset E8_SELFTEST_PANIC CDPATH OPENALPACA_HOME_STORE OPENALPACA_CONFIG_DIR

HERE=$(cd -P -- "$(dirname -- "$0")" && pwd -P)
SCRIPT=$HERE/rehearsal.sh
MARK=MARKER-e8-7f3a
BINS="" WORK=""
usage() { echo "usage: rehearsal-test.sh --bins <dir> [--work <dir>]" >&2; exit 2; }
while [ $# -gt 0 ]; do
  case "$1" in
    --bins) [ $# -ge 2 ] || usage; BINS=$2; shift 2 ;;
    --work) [ $# -ge 2 ] || usage; WORK=$2; shift 2 ;;
    *) usage ;;
  esac
done
[ -n "$BINS" ] && [ -f "$BINS/SHA256SUMS" ] || usage
BINS=$(cd -P -- "$BINS" && pwd -P)
if [ -z "$WORK" ]; then WORK=$(mktemp -d "${TMPDIR:-/tmp}/e8rht.XXXXXX"); fi
mkdir -p "$WORK"
X=$(cd -P -- "$WORK" && pwd -P)
echo "rehearsal-test: work dir $X"
echo "rehearsal-test: bins $BINS"

PASS=0
FAILS=0
check() {   # description command... — passes when the command exits 0
  local d=$1
  shift
  if "$@"; then
    PASS=$((PASS + 1))
    echo "ok   $d"
  else
    FAILS=$((FAILS + 1))
    echo "FAIL $d"
  fi
}

# ---- the corpus and the copies -------------------------------------------------------
/bin/bash "$HERE/probe-test.sh" --corpus-only "$X/corpus" >/dev/null
ORIG=$X/corpus/orig-$MARK/store
# copy <name>: $X/<name>/realcopy-$MARK, a `cp -Rp` of the original in a 0700
# folder whose own path holds no marker (the line after the block names the
# work dir beside the copy, so only the copy's own name carries one).
copy() {
  rm -rf "${X:?}/$1"
  mkdir -p "$X/$1"
  chmod 700 "$X/$1"
  cp -Rp "$ORIG" "$X/$1/realcopy-$MARK"
  printf '%s\n' "$X/$1/realcopy-$MARK"
}
state_of() {   # dir — every file below it with its hash, every entry with its stat
  (cd "$1" && { find . -type f -print0 | xargs -0 shasum -a 256; find . -exec stat -f '%N %HT %Lp %z %m %c %i' {} +; } | LC_ALL=C sort)
}

# rh <name> <args...>: rehearsal.sh's stdout, stderr and status in $X/runs/<name>.*
mkdir -p "$X/runs"
rh() {
  local name=$1 rc=0
  shift
  /bin/bash "$SCRIPT" "$@" >"$X/runs/$name.out" 2>"$X/runs/$name.err" </dev/null || rc=$?
  echo "$rc" >"$X/runs/$name.rc"
}
rc_is() { [ "$(cat "$X/runs/$1.rc")" = "$2" ]; }
has() { grep -q -x -F -- "$2" "$X/runs/$1.out"; }
framed() {   # stdout is the block alone: it starts and ends with the frame lines
  [ "$(head -n 1 "$X/runs/$1.out")" = "=== PASTE FROM HERE ===" ] && [ "$(tail -n 1 "$X/runs/$1.out")" = "=== TO HERE ===" ] &&
    [ "$(grep -c -x -F '=== TO HERE ===' "$X/runs/$1.out")" = 1 ]
}
trailer_only() {   # stderr is the one line naming the work dir
  [ "$(wc -l <"$X/runs/$1.err" | tr -d ' ')" = 1 ] && grep -q -E '^\(rehearsal, not for pasting: .*/e8-rehearsal\.[A-Za-z0-9]+ holds ' "$X/runs/$1.err"
}
clean_streams() {   # no marker on either stream, no .openalpaca and no dropped line in the block
  ! grep -q -F MARKER-e8 "$X/runs/$1.out" "$X/runs/$1.err" && ! grep -q -F .openalpaca "$X/runs/$1.out" &&
    ! grep -q '^dropped-lines' "$X/runs/$1.out"
}
refused() {   # name text — exit 2, nothing on stdout, the refusal and the usage line on stderr
  rc_is "$1" 2 && [ ! -s "$X/runs/$1.out" ] && grep -q -F -- "$2" "$X/runs/$1.err" &&
    [ "$(wc -l <"$X/runs/$1.err" | tr -d ' ')" = 2 ]
}
nothing_beside() {   # copy — no work dir made beside it, no sidecar made in it (a planted link is not one)
  [ -z "$(find "$(dirname "$1")" -maxdepth 1 -name 'e8-rehearsal.*' -print)" ] &&
    [ -z "$(find "$1" -type f \( -name '*-shm' -o -name '*-wal' \) -print)" ]
}

# ---- real: the real binaries ---------------------------------------------------------------
C=$(copy real)
state_of "$C/memory" >"$X/real-memory.before"
state_of "$C/state/memory-control" >"$X/real-control.before"
rh real --copy "$C" --bins "$BINS"
check "[real] exit 0, status complete, verdict pass" eval 'rc_is real 0 && has real "status complete" && has real "verdict pass"'
check "[real] stdout is the block alone" framed real
check "[real] stderr is the one line naming the work dir" trailer_only real
check "[real] no marker on either stream, no .openalpaca, nothing dropped" clean_streams real
check "[real] bins checked; e8probe-old on 3.51.1 and e8probe-new on 3.53.2" \
  eval 'has real "bins-checksums ok" && grep -q -x -F "e8probe-old e8probe	sqlite 3.51.1" "$X/runs/real.out" && grep -q -x -F "e8probe-new e8probe	sqlite 3.53.2" "$X/runs/real.out"'
check "[real] step 3: the probe first, then sqlite3, schema 4, the same row total" \
  eval 'has real "step3-probe db	schema-version 4" && has real "step3 schema-version 4" && has real "step3 tables=29 rows-total=10" && has real "step3 error-lines 0"'
check "[real] both doctors scanned locally, exit 0, nothing on stderr" \
  eval 'has real "doctor-prewave local" && has real "doctor-tip local" && grep -q -E "^doctor-prewave exit 0 stdout-lines [0-9]+ stderr-lines 0$" "$X/runs/real.out" && grep -q -E "^doctor-tip exit 0 stdout-lines [0-9]+ stderr-lines 0$" "$X/runs/real.out"'
check "[real] both doctors: home quarantined (relocation, R09), the project ready, equal; the inventory's one extra key is the earlier build's snapshot" \
  eval 'grep -q -E "^doctor-prewave partition home-[0-9a-f]{32} quarantined$" "$X/runs/real.out" && grep -q -E "^doctor-tip partition proj-[0-9a-f]{32}-[0-9a-f]{16} ready$" "$X/runs/real.out" && has real "doctor-partitions-equal yes" && has real "doctors-equal yes" && has real "doctor-prewave-vs-inventory missing=1 extra=0" && has real "doctor-tip-vs-inventory missing=1 extra=0"'
check "[real] the probe: old, new, old equal; the search compared twice, no difference" \
  eval 'has real "probe-new-vs-old equal" && has real "probe-old-vs-old equal" && [ "$(grep -c -E "^p[23]-(old|new) search	queries [0-9]+ with-results [1-9][0-9]* equal [0-9]+ different 0 error 0$" "$X/runs/real.out")" = 2 ]'
check "[real] copy-memory-unchanged yes, and the test's own hash of memory/ and the snapshots agrees" \
  eval 'has real "copy-memory-unchanged yes" && state_of "$C/memory" | cmp -s - "$X/real-memory.before" && state_of "$C/state/memory-control" | cmp -s - "$X/real-control.before"'
WD=$(find "$X/real" -maxdepth 1 -name 'e8-rehearsal.*' -type d | head -n 1)
check "[real] both doctors list the earlier build's project snapshot apart (its line stays in the work dir), and the probe reads its notes unlocated" \
  eval 'grep -q -F "carries no project id" "$WD/out/doctor-prewave.txt" && grep -q -F "carries no project id" "$WD/out/doctor-tip.txt" && grep -q -E "^p2-new snapshots	3	proj-[0-9a-f]{32}-f00dfacef00dface	project	.*	key=absent	folder=orphan	.*	missing=0	unlocated=2	" "$X/runs/real.out"'
check "[real] the work dir is beside the copy, 0700, and holds the doctors' full reports (which name paths)" \
  eval '[ -n "$WD" ] && [ "$(stat -f "%Lp" "$WD")" = 700 ] && grep -q -F "$C" "$WD/out/doctor-tip.txt"'
check "[real] the doctors ran with the work dir's HOME and the copy as the store (no lock or discovery file appeared in the copy)" \
  eval '[ -d "$WD/home" ] && [ ! -e "$C/state/discovery.json" ] && [ ! -e "$C/state/openalpacad.lock" ]'

# ---- no index ---------------------------------------------------------------------------------
C=$(copy noindex)
rm -rf "$C/state/cache"
rh noindex --copy "$C" --bins "$BINS"
check "[no index] exit 0, cache-copies index-absent, the three cache checks absent, verdict pass" \
  eval 'rc_is noindex 0 && has noindex "cache-copies index-absent" && has noindex "p2-new index	absent" && has noindex "p2-new fts-rw	absent" && has noindex "p2-new search	absent" && has noindex "verdict pass" && framed noindex && clean_streams noindex'

# ---- flipped byte --------------------------------------------------------------------------------
C=$(copy flipped)
F=$(find "$C/memory" -name '*--*.md' | LC_ALL=C sort | head -n 1)
python3 -I -c 'import sys; p=sys.argv[1]; b=bytearray(open(p,"rb").read()); b[-2] ^= 1; open(p,"wb").write(bytes(b))' "$F"
rh flipped --copy "$C" --bins "$BINS"
check "[flipped] exit 0, verdict fail naming note-mismatch, the doctors still local" \
  eval 'rc_is flipped 0 && grep -q -E "^verdict fail reasons=(.*,)?note-mismatch(,.*)?$" "$X/runs/flipped.out" && has flipped "doctor-tip local" && clean_streams flipped'

# ---- a non-ASCII byte in the copy's path ----------------------------------------------------------
C=$(copy "uri-"$'\xc3\xa9')
rh nonascii --copy "$C" --bins "$BINS"
check "[non-ASCII] a copy path holding a byte >= 0x80: step 3's sqlite3 reads it (schema 4, the same row total), verdict pass" \
  eval 'rc_is nonascii 0 && has nonascii "step3 schema-version 4" && has nonascii "step3 tables=29 rows-total=10" && has nonascii "step3 error-lines 0" && has nonascii "verdict pass" && framed nonascii && clean_streams nonascii'

# ---- no snapshot to compare -------------------------------------------------------------------------
C=$(copy nosnaps)
rm -rf "$C/state/memory-control"
rh nosnaps --copy "$C" --bins "$BINS"
check "[no snapshots] the copy without state/memory-control: verdict fail naming snapshots-absent and snapshots-vacuous" \
  eval 'rc_is nosnaps 0 && grep -q -E "^verdict fail reasons=(.*,)?snapshots-absent(,.*)?$" "$X/runs/nosnaps.out" && grep -q -E "^verdict fail reasons=(.*,)?snapshots-vacuous(,.*)?$" "$X/runs/nosnaps.out" && framed nosnaps && clean_streams nosnaps'

# ---- refusals -------------------------------------------------------------------------------------
mkdir -p "$X/refuse"
chmod 700 "$X/refuse"
cp -Rp "$ORIG" "$X/refuse/.openalpaca"
ln -s "$X/refuse/.openalpaca" "$X/refuse/link-$MARK"
rh dot --copy "$X/refuse/.openalpaca" --bins "$BINS"
check "[refuse] a .openalpaca component: exit 2, refused, nothing made" \
  eval 'refused dot "refused: --copy has a component named .openalpaca" && nothing_beside "$X/refuse/.openalpaca"'
rh dotlink --copy "$X/refuse/link-$MARK" --bins "$BINS"
check "[refuse] a .openalpaca component through a link: exit 2, refused, nothing made" \
  eval 'refused dotlink "refused: --copy resolves through a component named .openalpaca" && nothing_beside "$X/refuse/.openalpaca"'
rh dotcase --copy "$X/refuse/.OpenAlpaca" --bins "$BINS"
check "[refuse] a .OpenAlpaca component (another letter case): exit 2, refused, nothing made" \
  eval 'refused dotcase "refused: --copy has a component named .openalpaca" && nothing_beside "$X/refuse/.openalpaca"'
rh dotupper --copy="$X/refuse/.OPENALPACA/" --bins "$BINS"
check "[refuse] --copy=<...>/.OPENALPACA/: exit 2, refused, nothing made" \
  eval 'refused dotupper "refused: --copy has a component named .openalpaca" && nothing_beside "$X/refuse/.openalpaca"'
mkdir -p "$X/refuse-case"
chmod 700 "$X/refuse-case"
cp -Rp "$ORIG" "$X/refuse-case/.OpenAlpaca"
ln -s "$X/refuse-case/.OpenAlpaca" "$X/refuse-case/link-$MARK"
rh dotcaselink --copy "$X/refuse-case/link-$MARK" --bins "$BINS"
check "[refuse] a link to a folder named .OpenAlpaca on disk: exit 2, refused, nothing made" \
  eval 'refused dotcaselink "refused: --copy resolves through a component named .openalpaca" && nothing_beside "$X/refuse-case/.OpenAlpaca"'
C=$(copy linked-copy)
ln -s "$C" "$X/linked-copy/link-$MARK"
rh copylink --copy "$X/linked-copy/link-$MARK" --bins "$BINS"
check "[refuse] a copy that is itself a link (cp -Rp of a linked store): exit 2, refused, nothing made" \
  eval 'refused copylink "refused: --copy is a symbolic link" && nothing_beside "$C"'
rh copylinkslash --copy "$X/linked-copy/link-$MARK/" --bins "$BINS"
check "[refuse] the same link given with a trailing slash: exit 2, refused" \
  eval 'refused copylinkslash "refused: --copy is a symbolic link" && nothing_beside "$C"'
C=$(copy linked-state)
mkdir -p "$X/linked-state/elsewhere"
mv "$C/state" "$X/linked-state/elsewhere/state"
ln -s "$X/linked-state/elsewhere/state" "$C/state"
state_of "$X/linked-state/elsewhere" >"$X/linked-state.before"
rh lstate --copy "$C" --bins "$BINS"
check "[refuse] a copy whose state/ is a link: exit 2, refused, nothing made, the link's target untouched (no -wal, no -shm)" \
  eval 'refused lstate "refused: the copy holds 1 symbolic link(s)" && nothing_beside "$C" && state_of "$X/linked-state/elsewhere" | cmp -s - "$X/linked-state.before" && [ -z "$(find "$X/linked-state/elsewhere" \( -name "*-shm" -o -name "*-wal" \) -print)" ]'
C=$(copy linked-config)
ln -s "$X/nowhere-$MARK" "$C/config/llm.toml"
rh lconfig --copy "$C" --bins "$BINS"
check "[refuse] a copy holding a link two levels down (config/llm.toml): exit 2, refused, nothing made" \
  eval 'refused lconfig "refused: the copy holds 1 symbolic link(s)" && nothing_beside "$C"'
C=$(copy linked-sidecar)
ln -s "$X/nowhere-$MARK" "$C/state/cache/memory-index-v1.sqlite-wal"
rh lsidecar --copy "$C" --bins "$BINS"
check "[refuse] a copy holding a link two levels under state/ (a cache sidecar): exit 2, refused, nothing made" \
  eval 'refused lsidecar "refused: the copy holds 1 symbolic link(s)" && nothing_beside "$C"'
C=$(copy disc)
: >"$C/state/discovery.json"
rh disc --copy "$C" --bins "$BINS"
check "[refuse] a copy holding state/discovery.json: exit 2, refused, nothing made" \
  eval 'refused disc "refused: the copy holds state/discovery.json" && nothing_beside "$C"'
C=$(copy lock)
: >"$C/state/openalpacad.lock"
rh lock --copy "$C" --bins "$BINS"
check "[refuse] a copy holding state/openalpacad.lock: exit 2, refused, nothing made" \
  eval 'refused lock "refused: the copy holds state/openalpacad.lock" && nothing_beside "$C"'
C=$(copy locklink)
ln -s "$X/nowhere" "$C/state/openalpacad.lock"
rh locklink --copy "$C" --bins "$BINS"
check "[refuse] a copy holding state/openalpacad.lock as a dangling link: exit 2, refused as the copy's one link, nothing made" \
  eval 'refused locklink "refused: the copy holds 1 symbolic link(s)" && nothing_beside "$C"'
C=$(copy args)
rh nobins --copy "$C"
check "[refuse] no --bins: exit 2" eval 'refused nobins "--bins is required" && nothing_beside "$C"'
mkdir -p "$X/bins-short"
cp -p "$BINS/e8probe-old" "$BINS/e8probe-new" "$BINS/SHA256SUMS" "$X/bins-short/"
rh shortbins --copy "$C" --bins "$X/bins-short"
check "[refuse] --bins without the CLIs: exit 2" eval 'refused shortbins "--bins lacks an executable openalpaca-prewave" && nothing_beside "$C"'
rh unknown --copy "$C" --bins "$BINS" --frobnicate
check "[refuse] an unknown argument: exit 2" eval 'refused unknown "unknown argument" && nothing_beside "$C"'
rh help --help
check "[help] --help: exit 0, step 1's commands on stdout, stderr empty" \
  eval 'rc_is help 0 && [ ! -s "$X/runs/help.err" ] && grep -q -F "cp -Rp ~/.openalpaca \"\$T/realcopy\"" "$X/runs/help.out" && grep -q -F "rm -f \"\$T/realcopy/state/discovery.json\" \"\$T/realcopy/state/openalpacad.lock\"" "$X/runs/help.out" && grep -q -F "openalpaca daemon stop" "$X/runs/help.out"'
help_line() { grep -n -F -- "$1" "$X/runs/help.out" | head -n 1 | cut -d: -f1; }   # the first line holding the text
check "[help] step 1 checks the copy and its state/ for a link before the rm" \
  eval 'a=$(help_line "[ -L \"\$T/realcopy\" ]"); b=$(help_line "[ -L \"\$T/realcopy/state\" ]"); r=$(help_line "rm -f \"\$T/realcopy/state/discovery.json\""); [ -n "$a" ] && [ -n "$b" ] && [ -n "$r" ] && [ "$a" -lt "$r" ] && [ "$b" -lt "$r" ]'

# ---- the sums ----------------------------------------------------------------------------------------
fake_bins() {   # name — a copy of the real folder, its SHA256SUMS rewritten after the caller's change
  rm -rf "${X:?}/bins-$1"
  mkdir -p "$X/bins-$1"
  cp -p "$BINS"/e8probe-old "$BINS"/e8probe-new "$BINS"/openalpaca-prewave "$BINS"/openalpaca-tip "$X/bins-$1/"
}
resum() { (cd "$X/bins-$1" && shasum -a 256 e8probe-old e8probe-new openalpaca-prewave openalpaca-tip >SHA256SUMS); }
fake_bins tampered
resum tampered
printf 'x' >>"$X/bins-tampered/e8probe-new"
C=$(copy tampered)
rh tampered --copy "$C" --bins "$X/bins-tampered"
check "[sums] a binary changed after SHA256SUMS: exit 1, bins-checksums failed, the copy untouched" \
  eval 'rc_is tampered 1 && has tampered "bins-checksums failed" && has tampered "step-failed bins-checksums" && framed tampered && [ -z "$(find "$C" \( -name "*-shm" -o -name "*-wal" \) -print)" ] && [ -z "$(find "$X/tampered"/e8-rehearsal.*/cache-ro -type f -print)" ]'
fake_bins othername
resum othername
printf '%s  %s\n' "$(shasum -a 256 "$X/bins-othername/e8probe-old" | cut -d' ' -f1)" other >>"$X/bins-othername/SHA256SUMS"
C=$(copy othername)
rh othername --copy "$C" --bins "$X/bins-othername"
check "[sums] a SHA256SUMS naming a file the folder lacks: exit 1, bins-checksums failed" \
  eval 'rc_is othername 1 && has othername "bins-checksums failed" && [ -z "$(find "$C" \( -name "*-shm" -o -name "*-wal" \) -print)" ]'

fake_bins e7style
printf 'a stand-in for E7s pre-wave daemon\n' >"$X/bins-e7style/openalpacad-prewave"
(cd "$X/bins-e7style" && shasum -a 256 openalpacad-prewave e8probe-old e8probe-new openalpaca-prewave openalpaca-tip >SHA256SUMS)
C=$(copy e7style)
rh e7style --copy "$C" --bins "$X/bins-e7style"
check "[sums] E7's folder, its SHA256SUMS also listing openalpacad-prewave: bins-checksums ok, verdict pass" \
  eval 'rc_is e7style 0 && has e7style "bins-checksums ok" && has e7style "verdict pass"'

# ---- fake doctors ------------------------------------------------------------------------------------------
# A doctor as print_doctor prints one, from the real tip's report of the same
# copy: `void` drops the first line (the report a daemon sends), `differ`
# changes the project's health word, `hang` sleeps first.
REAL_REPORT=$(find "$X/real" -path '*/out/doctor-tip.txt' | head -n 1)
fake_doctor() {   # name mode
  fake_bins "$1"
  cp "$REAL_REPORT" "$X/bins-$1/report.txt"
  case "$2" in
    void) printf '#!/bin/sh\nsed 1d %s\n' "$X/bins-$1/report.txt" >"$X/bins-$1/openalpaca-tip" ;;
    differ) printf '#!/bin/sh\nsed "s/  ready\\$/  quarantined/" %s\n' "$X/bins-$1/report.txt" >"$X/bins-$1/openalpaca-tip" ;;
    hang) printf '#!/bin/sh\nsleep 30\ncat %s\n' "$X/bins-$1/report.txt" >"$X/bins-$1/openalpaca-tip" ;;
  esac
  chmod 755 "$X/bins-$1/openalpaca-tip"
  resum "$1"
}
fake_doctor void void
C=$(copy void)
rh void --copy "$C" --bins "$X/bins-void"
check "[void] a tip doctor without the first line: doctor-tip VOID, verdict void, exit 0" \
  eval 'rc_is void 0 && has void "doctor-tip VOID" && has void "doctor-prewave local" && grep -q -E "^verdict void \(doctor-tip\)" "$X/runs/void.out" && framed void && clean_streams void'
fake_doctor differ differ
C=$(copy differ)
rh differ --copy "$C" --bins "$X/bins-differ"
check "[differ] a tip doctor reading the project quarantined: doctor-partitions-equal no, verdict fail" \
  eval 'rc_is differ 0 && has differ "doctor-partitions-equal no" && grep -q -E "^verdict fail reasons=(.*,)?doctor-partitions(,.*)?$" "$X/runs/differ.out" && clean_streams differ'
fake_doctor hang hang
C=$(copy timeout)
rh timeout --copy "$C" --bins "$X/bins-hang" --tool-time-limit 2
check "[timeout] a hanging doctor under --tool-time-limit 2: step-failed doctor-tip-timeout, exit 1, the block printed" \
  eval 'rc_is timeout 1 && has timeout "step-failed doctor-tip-timeout" && has timeout "status incomplete steps-failed=1" && framed timeout && trailer_only timeout'
C=$(copy term)
# Its own process group (as a terminal's job is), sent TERM while the doctor hangs.
perl -e 'setpgrp(0, 0); exec @ARGV' /bin/bash "$SCRIPT" --copy "$C" --bins "$X/bins-hang" \
  >"$X/runs/term.out" 2>"$X/runs/term.err" </dev/null &
TPID=$!
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if pgrep -f "bins-hang/openalpaca-tip" >/dev/null 2>&1; then break; fi
  sleep 1
done
kill -TERM -- "-$TPID" 2>/dev/null || true
rc=0
wait "$TPID" || rc=$?
echo "$rc" >"$X/runs/term.rc"
check "[TERM] TERM to the process group while the doctor hangs: exit 143, the block printed, status incomplete" \
  eval 'rc_is term 143 && framed term && grep -q -E "^status incomplete steps-failed=[1-9]" "$X/runs/term.out" && grep -q -E "^step-failed doctor-tip-interrupted$" "$X/runs/term.out"'

# ---- the bins folder is never written ---------------------------------------------------------------------------
check "the handed-over bins folder still checks" eval '(cd "$BINS" && shasum -a 256 -c SHA256SUMS >/dev/null)'

echo "rehearsal-test: $PASS passed, $FAILS failed"
[ "$FAILS" = 0 ]
