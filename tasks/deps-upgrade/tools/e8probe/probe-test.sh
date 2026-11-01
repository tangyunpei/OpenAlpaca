#!/bin/bash
# probe-test.sh — E8-PROBE's test on synthetic stores only (README.md here,
# *How it is tested*; programme-plan §4.5 E8-PROBE). It builds corpus B under a
# fresh `mktemp -d` — the database from the tracked migrations through
# /usr/bin/sqlite3, the index from memory_index/mod.rs's SCHEMA_SQL, snapshots
# and notes written here with their revisions computed — and runs both
# variants over it and over altered copies of it. It never reads or names a
# real store, starts no daemon and reaches no network.
#
#   /bin/bash tasks/deps-upgrade/tools/e8probe/probe-test.sh --old <e8probe-old> --new <e8probe-new> [--work <dir>]
#   /bin/bash tasks/deps-upgrade/tools/e8probe/probe-test.sh --build-root <dir> [--work <dir>]
#   /bin/bash tasks/deps-upgrade/tools/e8probe/probe-test.sh --corpus-only <dir>
#
# --build-root builds both variants first, as the README's Build block does,
# into <dir>/target/{old,new} (CARGO_TARGET_DIR and CARGO_BUILD_BUILD_DIR the
# same path, `--release --locked`, the toolchain this tree's
# rust-toolchain.toml pins). --corpus-only writes corpus B's original store
# and its `cp -Rp` copy under <dir> and exits (rehearsal-test.sh uses it).
# Exit 0 when every check passes, 1 otherwise, 2 for a usage error.
#
# The cases, each on both variants (programme-plan §4.5 E8-PROBE's gate; the
# README's table):
#   old equals new   every word and count line equal between the variants
#                    (old records the search, new compares, old compares
#                    again), sqlite 3.51.1 against 3.53.2, mismatches 0, each
#                    live snapshot `key=equal` and an earlier build's (no
#                    project id) `key=absent`, `folder=orphan`, its notes
#                    `unlocated`, not `missing`; search `different 0` with
#                    results; the inventory equal too
#   flipped byte     one note's byte changed: `mismatched=1` in its partition
#   missing file     one note deleted: `missing=1`
#   recorded root    every snapshot's `root` rewritten to a decoy folder of
#                    notes with other revisions: the counts equal the clean
#                    run's with the folder readable (a probe that followed
#                    `root` would read the decoys) and chmod 000; and under
#                    sandbox-exec denying file-read* on it, with `log stream`
#                    catching a calibration read and no e8probe read
#   markers          MARKER-e8 in note titles and bodies, a project folder's
#                    name, the snapshots' owner_id, a file_assets.project_root,
#                    a table row and the store's own path: never on stdout or
#                    stderr; every stdout line inside rehearsal.sh's alphabet
#   WAL              a row committed to an un-checkpointed WAL: rows-total +1,
#                    while sqlite3 with immutable=1 counts one fewer (R08);
#                    the database and its -wal byte-identical across the reads
#   FTS corruption   one fts_word_data block zeroed in a read-write copy:
#                    fts-rw `fts_word corrupt`, index `integrity not-ok`
#   ro changes none  every file of the input hashed before and after db,
#                    index, search and snapshots: main files and -wal
#                    unchanged; a -shm may change or appear (README)
#   refusals         a .openalpaca component (as given, in another letter case
#                    and through a link), a store holding state/discovery.json,
#                    one holding state/openalpacad.lock, a store whose state/
#                    is a link, a database or cache given through a link (its
#                    folder's or its own), --cache-rw under state/, the same
#                    file as --cache-ro or as the store's own database or index,
#                    or with another hard link, an existing --out: exit 2, one
#                    fixed stderr line, nothing opened (no -shm made, in the
#                    link's target either)
#   projects         the daemon's list(): a hidden folder passed over, two
#                    folders sharing an id both left out (their partition's
#                    notes unlocated), a project.json without created_at or
#                    with a relative root unreadable
#   .history         a link or a file at the owner folder's .history refuses
#                    the partition in the daemon: notes-dir=link-skipped
#   panic path       E8_SELFTEST_PANIC=<check>, the marker in its payload:
#                    `<check>\tpanic`, stderr only the fixed line
# The sha2 known answer is each variant's `cargo test --locked` (README).

# The checks below are shell strings run by eval, so their variables expand
# late (SC2016, and SC2034 for PANIC_LINE, read only inside them); the
# self-test knob is exported inside a subshell on purpose, so that it reaches
# that one run alone (SC2030, SC2031); probe_reads_reported is called only
# inside an eval'd check (SC2329).
# shellcheck disable=SC2016,SC2030,SC2031,SC2034,SC2329

set -euo pipefail
export LC_ALL=C
unset E8_SELFTEST_PANIC CDPATH OPENALPACA_HOME_STORE OPENALPACA_CONFIG_DIR

HERE=$(cd -P -- "$(dirname -- "$0")" && pwd -P)
R=$(git -C "$HERE" rev-parse --show-toplevel)
MIG=$R/crates/openalpaca_storage/src/migrations
IDX_RS=$R/crates/openalpaca_storage/src/memory_index/mod.rs
SQLITE=/usr/bin/sqlite3
MARK=MARKER-e8-7f3a
OWNER_ID="e8-owner-$MARK"
PROJECT_ID=5b0c2d6e-7a41-4f3b-9e2a-1c3d4e5f6a7b
PANIC_LINE='e8probe: a panic; its message is withheld because it could quote the input'

usage() {
  echo "usage: probe-test.sh (--old <bin> --new <bin> | --build-root <dir>) [--work <dir>] | --corpus-only <dir>" >&2
  exit 2
}
OLD="" NEW="" BUILD_ROOT="" WORK="" CORPUS_ONLY=""
while [ $# -gt 0 ]; do
  case "$1" in
    --old) [ $# -ge 2 ] || usage; OLD=$2; shift 2 ;;
    --new) [ $# -ge 2 ] || usage; NEW=$2; shift 2 ;;
    --build-root) [ $# -ge 2 ] || usage; BUILD_ROOT=$2; shift 2 ;;
    --work) [ $# -ge 2 ] || usage; WORK=$2; shift 2 ;;
    --corpus-only) [ $# -ge 2 ] || usage; CORPUS_ONLY=$2; shift 2 ;;
    *) usage ;;
  esac
done
[ -x "$SQLITE" ] || { echo "probe-test: $SQLITE is needed" >&2; exit 2; }
# The system sqlite3 as rehearsal.sh runs it: `env -i`, an empty HOME (no
# ~/.sqliterc can change the list output read below), -batch.
sq() { env -i PATH=/usr/bin:/bin HOME=/var/empty LC_ALL=C "$SQLITE" -batch "$@"; }

# ---- corpus B --------------------------------------------------------------------
# The index schema, verbatim from memory_index/mod.rs between its anchors:
# the line `const SCHEMA_SQL: &str = r#"` (exactly one) and the first `"#;`
# after it; then create_schema's rows (meta schema_version and state, the two
# `rank` settings whose values are the file's RANK_WEIGHTS and
# HEAD_RANK_WEIGHTS lines, each matched exactly once). The schema_version is
# 6, the pre-wave build's SCHEMA_VERSION and so the owner's copy's, not the
# tip's 7 (the sweep's S12 changed no table: SCHEMA_SQL and the weights are
# the same at the branch point and the tip).
index_schema() {   # -> stdout
  local n
  n=$(grep -c -x -F 'const SCHEMA_SQL: &str = r#"' "$IDX_RS" || true)
  [ "$n" = 1 ] || { echo "probe-test: SCHEMA_SQL's start anchor matched $n times" >&2; return 1; }
  awk 'f && $0 == "\"#;" { done = 1; exit } f { print } $0 == "const SCHEMA_SQL: &str = r#\"" { f = 1 } END { exit !done }' "$IDX_RS" ||
    { echo "probe-test: SCHEMA_SQL's end anchor not found" >&2; return 1; }
  local rw hw
  [ "$(grep -c -E '^const RANK_WEIGHTS: &str = "[^"]*";$' "$IDX_RS")" = 1 ] || return 1
  [ "$(grep -c -E '^const HEAD_RANK_WEIGHTS: &str = "[^"]*";$' "$IDX_RS")" = 1 ] || return 1
  rw=$(sed -n 's/^const RANK_WEIGHTS: &str = "\([^"]*\)";$/\1/p' "$IDX_RS")
  hw=$(sed -n 's/^const HEAD_RANK_WEIGHTS: &str = "\([^"]*\)";$/\1/p' "$IDX_RS")
  printf "INSERT OR REPLACE INTO meta(key, value) VALUES ('schema_version', '6');\n"
  printf "INSERT OR REPLACE INTO meta(key, value) VALUES ('state', 'ready');\n"
  printf "INSERT INTO fts_word(fts_word, rank) VALUES ('rank', '%s');\n" "$rw"
  printf "INSERT INTO fts_tri_head(fts_tri_head, rank) VALUES ('rank', '%s');\n" "$hw"
}

# The notes, snapshots, project.json and the index's rows, written by Python
# with exact bytes; the revisions are sha256 over each file's bytes
# (memory_files/note.rs `revision_of`, hash.rs `sha256_hex`).
# A quoted heredoc: the code holds both kinds of quote.
GEN_PY=$(cat <<'PY'
import hashlib, json, os, sys
store, orig, owner_id, project_id, project_root, mark, sqlout = sys.argv[1:8]
# The project folder's name is store/projects.rs `folder_name` of its root:
# every character but a letter or a digit one `-` (an ASCII root of at most
# 200 bytes, so no NFC step and no hash suffix), so the doctors' lookup finds
# the project by its root, as a registered project's.
assert project_root.isascii() and len(project_root) <= 200, "project root too long or not ASCII"
pdir = "".join(c if c.isalnum() else "-" for c in project_root)
h = lambda b: hashlib.sha256(b).hexdigest()
ok = h(owner_id.encode())[:32]
pk = h(project_id.encode())[:16]
TS = "2026-10-01T00:00:00Z"
def note(nid, title, body):
    fm = ["---", "schema: 1", "id: " + nid, "title: " + title, "kind: fact", "status: active",
          "fact_key: null", "aliases: []", "tags: []", "created_at: " + TS, "updated_at: " + TS,
          "observed_at: null", "valid_from: null", "valid_to: null", "review_after: null",
          "pinned: false", "sources: []", "supersedes: []", "---"]
    return ("\n".join(fm) + "\n# " + title + "\n\n## Knowledge\n\n" + body + "\n").encode()
def slug(t):
    return "-".join("".join(c if c.isalnum() else " " for c in t.lower()).split())[:48]
home = [("1a2b3c4d-0000-4000-8000-000000000001", "Prefers tabs over spaces " + mark, "Tabs everywhere " + mark),
        ("1a2b3c4d-0000-4000-8000-000000000002", "Memory notes are lexical", "No embedder, ever."),
        ("1a2b3c4d-0000-4000-8000-000000000003", "User works on the beta", "The beta waits for B1 to B3.")]
proj = [("1a2b3c4d-0000-4000-8000-000000000011", "Project uses sqlite " + mark, "SQLite 3.53.2 in the wave " + mark),
        ("1a2b3c4d-0000-4000-8000-000000000012", "Deploy procedure for the copy", "Stop, copy, probe.")]
tomb = "1a2b3c4d-0000-4000-8000-0000000000ff"
parts = [("home-" + ok, "home", os.path.join(store, "memory", "owners", ok, "notes"),
          os.path.join(orig, "memory", "owners", ok), home, None),
         ("proj-%s-%s" % (ok, pk), "project", os.path.join(store, "projects", pdir, "memory", "owners", ok, "notes"),
          os.path.join(orig, "projects", pdir, "memory", "owners", ok), proj, project_id)]
sql = []
q = lambda s: "NULL" if s is None else "'" + s.replace("'", "''") + "'"
rowid = 0
for key, scope, notes_dir, root, notes, pid in parts:
    os.makedirs(notes_dir, mode=0o700, exist_ok=True)
    recs = {}
    for nid, title, body in notes:
        b = note(nid, title, body)
        name = "%s--%s.md" % (nid, slug(title))
        with open(os.path.join(notes_dir, name), "wb") as f:
            f.write(b)
        recs[nid] = {"revision": h(b), "origin": "user", "written_at": TS}
        rowid += 1
        norm = " ".join(title.lower().split())
        sql.append("INSERT INTO note (rowid, partition_key, id, scope, revision, relative_path, title, norm_title, aliases, tags, norm_aliases, norm_tags, kind, status, effective_status, origin, authority, fact_key, valid_from, valid_to, updated_at, created_at, observed_at, date, pinned, source_forgotten, conflict, same_key_loser, superseded_by, supersedes, body_line_offset, mtime_ns, size, inode) VALUES (%d, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, 5, NULL, NULL, NULL, %s, %s, NULL, %s, 0, 0, 0, 0, %s, %s, 19, 0, %d, 0);"
                   % (rowid, q(key), q(nid), q("global" if scope == "home" else "project"), q(h(b)), q("notes/" + name), q(title), q(norm), q("[]"), q("[]"), q(""), q(""), q("fact"), q("active"), q("active"), q("user"), q(TS), q(TS), q(TS[:10]), q("[]"), q("[]"), len(b)))
        text = "## Knowledge\n\n" + body
        sql.append("INSERT INTO chunk (id, note_rowid, partition_key, ordinal, heading, line_from, line_to, body) VALUES (%d, %d, %s, 0, %s, 20, 23, %s);" % (rowid, rowid, q(key), q("Knowledge"), q(text)))
        sql.append("INSERT INTO fts_word (rowid, title, aliases, tags, body) VALUES (%d, %s, %s, %s, %s);" % (rowid, q(title), q(""), q(""), q(text)))
        sql.append("INSERT INTO fts_tri_head (rowid, title, aliases, tags) VALUES (%d, %s, %s, %s);" % (rowid, q(title), q(""), q("")))
        sql.append("INSERT INTO fts_tri (rowid, body) VALUES (%d, %s);" % (rowid, q(text)))
        sql.append("INSERT INTO note_alias (note_rowid, norm_alias) VALUES (%d, %s);" % (rowid, q(norm.split()[0])))
        sql.append("INSERT INTO note_source (note_rowid, source_key) VALUES (%d, %s);" % (rowid, q("message:e8-" + nid[-2:])))
    sql.append("INSERT INTO partition (partition_key, last_scan_at, last_scan_files) VALUES (%s, %s, %d);" % (q(key), q(TS), len(notes)))
    snap = {"schema": 1, "partition_key": key, "scope": scope, "owner_key": ok, "owner_id": owner_id, "root": root}
    if pid:
        snap["project_id"] = pid
    snap.update({"created_at": TS, "updated_at": TS, "moved_from": None, "repaired_at": None, "pending": None,
                 "notes": dict(sorted(recs.items())),
                 "tombstones": [{"id": tomb, "origin": "user", "supersedes": [], "suppressed_sources": [], "forgotten_at": TS}] if scope == "home" else []})
    with open(os.path.join(store, "state", "memory-control", key + ".json"), "w") as f:
        f.write(json.dumps(snap, indent=2) + "\n")
# An earlier build's project snapshot (R22): no project_id, keyed by the
# directory it was bound to, its folder long gone; the doctors list it apart
# and never open it, and its notes cannot be located.
ikey = "proj-%s-%s" % (ok, "f00dfacef00dface")
idless = {"schema": 1, "partition_key": ikey, "scope": "project", "owner_key": ok, "owner_id": owner_id,
          "root": "/nonexistent/" + mark + "/an-earlier-build/memory/owners/" + ok,
          "created_at": TS, "updated_at": TS, "moved_from": None, "repaired_at": None, "pending": None,
          "notes": {"1a2b3c4d-0000-4000-8000-000000000021": {"revision": h(b"gone 1"), "origin": "user", "written_at": TS},
                    "1a2b3c4d-0000-4000-8000-000000000022": {"revision": h(b"gone 2"), "origin": "user", "written_at": TS}},
          "tombstones": []}
with open(os.path.join(store, "state", "memory-control", ikey + ".json"), "w") as f:
    f.write(json.dumps(idless, indent=2) + "\n")
with open(os.path.join(store, "projects", pdir, "project.json"), "w") as f:
    f.write(json.dumps({"version": 1, "id": project_id, "root": project_root, "created_at": TS}, indent=2) + "\n")
with open(sqlout, "w") as f:
    f.write("\n".join(sql) + "\n")
PY
)

# The database's rows: the owner id the doctor reads (identity.local_user_id,
# repository/config), its global_user row, a row holding the marker, and three
# file_assets rows — an in-project pointer whose project_root names a path
# that does not exist, a retryable `error` and an `uploaded`.
db_rows() {   # store
  local s=$1
  cat <<EOF
PRAGMA journal_mode=WAL;
INSERT INTO system_config (key, value, kind) VALUES ('identity.local_user_id', '$OWNER_ID', 'string');
INSERT INTO system_config (key, value, kind) VALUES ('e8.marker', 'a row holding $MARK', 'string');
INSERT INTO global_user (id, display_name) VALUES ('$OWNER_ID', 'Local User');
INSERT INTO file_assets (id, owner_id, sha256, filename, mime_type, size_bytes, storage_path, status, origin, project_root, rel_path, in_project)
  VALUES ('fa-1', '$OWNER_ID', '$(printf 'a%.0s' $(seq 64))', 'a.txt', 'text/plain', 1, '/nonexistent/$MARK/a.txt', 'ready', 'produced', '/nonexistent/$MARK', 'a.txt', 1);
INSERT INTO file_assets (id, owner_id, sha256, filename, mime_type, size_bytes, storage_path, status, extract_error)
  VALUES ('fa-2', '$OWNER_ID', '$(printf 'b%.0s' $(seq 64))', 'b.pdf', 'application/pdf', 2, '$s/uploads/b.pdf', 'error', '[attempt:1] extraction failed');
INSERT INTO file_assets (id, owner_id, sha256, filename, mime_type, size_bytes, storage_path, status)
  VALUES ('fa-3', '$OWNER_ID', '$(printf 'c%.0s' $(seq 64))', 'c.txt', 'text/plain', 3, '$s/uploads/c.txt', 'uploaded');
EOF
}

# make_corpus <dir>: <dir>/orig-$MARK/store, the "real" store (its snapshots'
# roots name it), and <dir>/copy-$MARK/store, its `cp -Rp`, the copy the
# probe reads; the project's own folder, <dir>/projroot-$MARK, lies outside
# both, as a real project's does. Prints the copy's path.
make_corpus() {
  local d o c
  d=$(cd -P -- "$1" && pwd -P)
  o=$d/orig-$MARK/store
  c=$d/copy-$MARK/store
  mkdir -p "$o/state/memory-control" "$o/state/cache" "$o/memory/owners" "$o/projects" \
    "$o/config" "$o/uploads" "$d/copy-$MARK" "$d/projroot-$MARK"
  chmod 700 "$o/state"
  cat "$MIG"/001_*.sql "$MIG"/002_*.sql "$MIG"/003_*.sql "$MIG"/004_*.sql | sq "$o/state/openalpaca.db"
  db_rows "$o" | sq "$o/state/openalpaca.db" >/dev/null
  python3 -I -c "$GEN_PY" "$o" "$o" "$OWNER_ID" "$PROJECT_ID" "$d/projroot-$MARK" "$MARK" "$d/index-rows.sql"
  { echo "PRAGMA journal_mode=WAL;"; index_schema; cat "$d/index-rows.sql"; } |
    sq "$o/state/cache/memory-index-v1.sqlite" >/dev/null
  # The system sqlite3 keeps a checkpointed, zero-length -wal and its -shm
  # after the last close (persistent WAL); the daemon's bundled SQLite removes
  # both on a clean close, so a stopped daemon's store has neither. Removed
  # here so the corpus is that store; refused if a -wal still holds frames.
  local f
  for f in "$o/state/openalpaca.db" "$o/state/cache/memory-index-v1.sqlite"; do
    if [ -s "$f-wal" ]; then echo "probe-test: $f-wal is not empty" >&2; return 1; fi
    rm -f "$f-wal" "$f-shm"
  done
  cp -Rp "$o" "$c"
  printf '%s\n' "$c"
}

if [ -n "$CORPUS_ONLY" ]; then
  mkdir -p "$CORPUS_ONLY"
  make_corpus "$CORPUS_ONLY"
  exit 0
fi

# ---- the binaries -----------------------------------------------------------------
if [ -n "$BUILD_ROOT" ]; then
  [ -z "$OLD$NEW" ] || usage
  mkdir -p "$BUILD_ROOT"
  BUILD_ROOT=$(cd -P -- "$BUILD_ROOT" && pwd -P)
  TC=$(sed -n 's/^channel *= *"\(.*\)"$/\1/p' "$R/rust-toolchain.toml")
  for v in old new; do
    CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0 CARGO_HTTP_USER_AGENT=openalpaca-deps-check \
      CARGO_TARGET_DIR="$BUILD_ROOT/target/$v" CARGO_BUILD_BUILD_DIR="$BUILD_ROOT/target/$v" \
      cargo "+$TC" build --release --locked --manifest-path "$HERE/$v/Cargo.toml"
  done
  OLD=$BUILD_ROOT/target/old/release/e8probe
  NEW=$BUILD_ROOT/target/new/release/e8probe
fi
[ -x "$OLD" ] && [ -x "$NEW" ] || usage

if [ -z "$WORK" ]; then WORK=$(mktemp -d "${TMPDIR:-/tmp}/e8probe-test.XXXXXX"); fi
mkdir -p "$WORK"
X=$(cd -P -- "$WORK" && pwd -P)
RUNS=$X/runs
mkdir -p "$RUNS"
echo "probe-test: work dir $X"
echo "probe-test: old $OLD"
echo "probe-test: new $NEW"

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

# run <name> <old|new> <args...>: the variant's stdout, stderr and status in
# $RUNS/<name>.{out,err,rc}.
run() {
  local name=$1 v=$2 bin rc=0
  shift 2
  if [ "$v" = old ]; then bin=$OLD; else bin=$NEW; fi
  "$bin" "$@" >"$RUNS/$name.out" 2>"$RUNS/$name.err" </dev/null || rc=$?
  echo "$rc" >"$RUNS/$name.rc"
}
rc_is() { [ "$(cat "$RUNS/$1.rc")" = "$2" ]; }
has() { grep -q -x -F -- "$2" "$RUNS/$1.out"; }
err_empty() { [ ! -s "$RUNS/$1.err" ]; }
err_is() { [ "$(wc -l <"$RUNS/$1.err" | tr -d ' ')" = 1 ] && grep -q -x -F -- "$2" "$RUNS/$1.err"; }
no_marker() {   # name... — no MARKER-e8 on either stream of each run
  local n
  for n in "$@"; do
    if grep -q -F MARKER-e8 "$RUNS/$n.out" "$RUNS/$n.err"; then return 1; fi
  done
}
# Every stdout line inside rehearsal.sh's paste alphabet ([A-Za-z0-9 ._:=(),+-]
# plus tab, under LC_ALL=C): nothing the rehearsal would drop.
in_alphabet() { ! grep -q -v -E $'^[A-Za-z0-9 ._:=(),+\t-]*$' "$RUNS/$1.out"; }
# The run's lines without the ones that name the SQLite version.
words() { grep -v -E $'(^|\t)(all )?sqlite [0-9.]+$' "$RUNS/$1.out" || true; }
same_words() { [ "$(words "$1")" = "$(words "$2")" ]; }
field() {   # name line-prefix key — the value of key=<v> on the first line starting with prefix
  awk -F '\t' -v p="$2" -v k="$3" 'index($0, p) == 1 { for (i = 1; i <= NF; i++) if (index($i, k "=") == 1) { print substr($i, length(k) + 2); exit } }' "$RUNS/$1.out"
}
hashes() {   # dir — every regular file below it, with its sha256 (relative names), sorted
  (cd "$1" && find . -type f -print0 | xargs -0 shasum -a 256) | LC_ALL=C sort
}
fresh() {   # <name> — a new cp -Rp of the original store (no reader has opened it), its path printed
  local t=$X/$1-$MARK
  rm -rf "$t"
  mkdir -p "$t"
  cp -Rp "$ORIG" "$t/store"
  printf '%s\n' "$t/store"
}
step3() {   # store out — E8 step 3's block as rehearsal.sh runs it (-safe, quotes doubled); 1 when a call fails
  local DB="file:$1/state/openalpaca.db?mode=ro" t q n
  {
    sq -safe "$DB" 'SELECT max(version) FROM schema_version;' || return 1
    sq -safe "$DB" "SELECT type, name, sql FROM sqlite_master ORDER BY type, name;" || return 1
    sq -safe "$DB" "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name;" >"$2.tables" || return 1
    while read -r t; do
      q=${t//\"/\"\"}
      n=$(sq -safe "$DB" "SELECT count(*) FROM \"$q\";") || return 1
      printf '%s\t%s\n' "$t" "$n"
    done <"$2.tables"
    sq -safe "$DB" 'SELECT count(*), count(DISTINCT project_root) FROM file_assets WHERE in_project = 1;' || return 1
    sq -safe "$DB" "SELECT count(*) FROM file_assets WHERE status = 'error';" || return 1
  } >"$2" 2>&1
}

# ---- the corpus ---------------------------------------------------------------------
mkdir -p "$X/corpus"
A=$(make_corpus "$X/corpus")
ORIG=$X/corpus/orig-$MARK/store
# The corpus checked through immutable=1 on the original: a test-only read of
# files with no WAL frames (make_corpus refused any), so nothing is skipped;
# WAL mode read from each file's header (bytes 18 and 19 both 2).
check "corpus B: a schema-4 database in WAL mode, the index at schema 6, three snapshots, five notes; no -wal, no -shm" \
  test "$(sq "file:$ORIG/state/openalpaca.db?immutable=1" 'SELECT max(version) FROM schema_version;')" = 4 -a \
  "$(python3 -I -c 'import sys; print(all(open(f, "rb").read(20)[18:20] == bytes([2, 2]) for f in sys.argv[1:]))' \
    "$ORIG/state/openalpaca.db" "$ORIG/state/cache/memory-index-v1.sqlite")" = True -a \
  "$(sq "file:$ORIG/state/cache/memory-index-v1.sqlite?immutable=1" "SELECT value FROM meta WHERE key='schema_version'; SELECT count(*) FROM note; SELECT count(*) FROM fts_word_data;" | head -2 | tr '\n' ' ')" = "6 5 " -a \
  -z "$(find "$A" \( -name '*-wal' -o -name '*-shm' \) -print)" -a \
  "$(find "$A/state/memory-control" -name '*.json' | wc -l | tr -d ' ')" = 3 -a \
  "$(find "$A/memory" "$A/projects" -name '*--*.md' | wc -l | tr -d ' ')" = 5
check "corpus B: a recorded revision equals shasum -a 256 of its file" \
  test "$(shasum -a 256 "$(find "$A/memory" -name '*--*.md' | LC_ALL=C sort | head -1)" | cut -d' ' -f1)" = \
  "$(python3 -I -c 'import json,sys; s=json.load(open(sys.argv[1])); print(sorted(s["notes"].items())[0][1]["revision"])' "$(find "$A/state/memory-control" -name 'home-*.json')")"
# A cleanly stopped daemon's store has no -wal and no -shm (its bundled
# SQLite removes both at the last close). The system sqlite3 (3.54.0) cannot
# open such a WAL database read-only — "unable to open database file" — while
# the probe's bundled SQLite creates a zero-length -wal and a -shm beside it
# and reads it; after that, sqlite3 reads it too. So rehearsal.sh's step 3
# runs the probe's db check first (the plan's "or the rebuilt probe in
# mode=ro"), then the sqlite3 block.
check "a WAL database with no -wal or -shm: the system sqlite3 cannot open it read-only" \
  eval '! sq "file:$A/state/openalpaca.db?mode=ro" "SELECT 1;" >/dev/null 2>&1'
run step3-probe new db --db "$A/state/openalpaca.db" --out "$X/step3-probe.txt"
check "the probe opens it read-only, leaving a zero-length -wal and a -shm" \
  eval 'rc_is step3-probe 0 && [ -f "$A/state/openalpaca.db-wal" ] && [ ! -s "$A/state/openalpaca.db-wal" ] && [ -f "$A/state/openalpaca.db-shm" ]'
check "then step 3's sqlite3 block runs, every call exiting 0" step3 "$A" "$X/step3.txt"
check "and reads it, schema 4 first" test "$(head -1 "$X/step3.txt")" = 4
CR=$X/cache-ro
mkdir -p "$CR" "$X/cache-rw-old" "$X/cache-rw-new"
for d in "$CR" "$X/cache-rw-old" "$X/cache-rw-new"; do cp -p "$A/state/cache/"memory-index-v1.sqlite* "$d/"; done
CRO=$CR/memory-index-v1.sqlite
mkdir -p "$X/out"

# ---- old equals new -------------------------------------------------------------------
rm -f "$X/out/search.record"
n=0
for v in old new old; do
  n=$((n + 1))
  run "all-$n-$v" "$v" all --store "$A" --cache-ro "$CRO" --cache-rw "$X/cache-rw-$v/memory-index-v1.sqlite" \
    --step3 "$X/step3.txt" --record "$X/out/search.record" --out "$X/out/all-$n-$v.txt"
done
for r in all-1-old all-2-new all-3-old; do
  check "[$r] exit 0, stderr empty" rc_is "$r" 0
  check "[$r] stderr empty" err_empty "$r"
  check "[$r] db integrity ok, fk 0, schema-version 4, schema-set 1,2,3,4, addr-index expected, step3-counts equal" \
    eval 'has '"$r"' "db	integrity ok" && has '"$r"' "db	fk 0" && has '"$r"' "db	schema-version 4" && has '"$r"' "db	schema-set 1,2,3,4" && has '"$r"' "db	addr-index expected" && has '"$r"' "db	step3-counts equal"'
  check "[$r] db in-project rows=1 roots=1, extraction error=1 uploaded=1" \
    eval 'has '"$r"' "db	in-project rows=1 roots=1" && has '"$r"' "db	extraction error=1 uploaded=1 processing=0"'
  check "[$r] index integrity ok, schema 6, state ready, fts-tables 3, rows" \
    eval 'has '"$r"' "index	integrity ok" && has '"$r"' "index	schema 6" && has '"$r"' "index	state ready" && has '"$r"' "index	fts-tables 3" && has '"$r"' "index	rows note=5 chunk=5 note_alias=5 note_source=5 partition=2"'
  check "[$r] fts-rw ok on all three tables" \
    eval 'has '"$r"' "fts-rw	fts_word ok" && has '"$r"' "fts-rw	fts_tri_head ok" && has '"$r"' "fts-rw	fts_tri ok"'
  check "[$r] snapshots: three, none key-different, matched=5, no mismatch, missing or unrecorded" \
    test "$(field "$r" "snapshots	total" key-different)/$(field "$r" "snapshots	total" matched)/$(field "$r" "snapshots	total" mismatched)/$(field "$r" "snapshots	total" missing)/$(field "$r" "snapshots	total" unrecorded)/$(field "$r" "snapshots	total" count)" = "0/5/0/0/0/3"
  check "[$r] the two live snapshots key=equal; the earlier build's (no project id) key=absent, folder=orphan, its 2 notes unlocated, not missing" \
    test "$(field "$r" "snapshots	1	" key)/$(field "$r" "snapshots	2	" key)/$(field "$r" "snapshots	3	" key)/$(field "$r" "snapshots	3	" folder)/$(field "$r" "snapshots	3	" unlocated)/$(field "$r" "snapshots	3	" missing)/$(field "$r" "snapshots	total" orphan)/$(field "$r" "snapshots	total" key-absent)" = "equal/equal/absent/orphan/2/0/1/1"
  check "[$r] markers on neither stream" no_marker "$r"
  check "[$r] every stdout line inside the paste alphabet" in_alphabet "$r"
done
check "[old] sqlite 3.51.1" has all-1-old "db	sqlite 3.51.1"
check "[new] sqlite 3.53.2" has all-2-new "db	sqlite 3.53.2"
check "[old record] search recorded with results" grep -q -E $'^search\trecorded queries [0-9]+ with-results [1-9][0-9]* error 0$' "$RUNS/all-1-old.out"
for r in all-2-new all-3-old; do
  check "[$r] search compared: query-list equal, different 0, error 0, with-results > 0" \
    eval 'has '"$r"' "search	query-list equal" && grep -q -E "^search	queries [0-9]+ with-results [1-9][0-9]* equal [0-9]+ different 0 error 0$" "$RUNS/'"$r"'.out"'
done
check "old equals new: every word and count line equal (new against the second old run)" same_words all-2-new all-3-old
check "old equals new: the record run differs only in its search lines" \
  test "$(words all-1-old | grep -v $'^search\t')" = "$(words all-2-new | grep -v $'^search\t')"
for v in old new; do
  run "inventory-$v" "$v" inventory --store "$A" --out "$X/out/inventory-$v.tsv"
  check "[$v] inventory: exit 0, 2 partitions with both folder and snapshot, the earlier build's snapshot alone, 1 readable project" \
    eval 'rc_is inventory-'"$v"' 0 && err_empty inventory-'"$v"' && has inventory-'"$v"' "inventory	partitions total=3 both=2 folder-only=0 snapshot-only=1" && has inventory-'"$v"' "inventory	projects folders=1 readable=1 unreadable=0 shared-id=0 hidden=0 link-skipped=0 other=0" && grep -q -E "^inventory	snapshot	3	proj-[0-9a-f]{32}-f00dfacef00dface	project	notes=2	tombstones=0	project-id=absent$" "$RUNS/inventory-'"$v"'.out"'
  check "[$v] inventory: no marker, inside the alphabet" eval 'no_marker inventory-'"$v"' && in_alphabet inventory-'"$v"''
done
check "old equals new: the inventory" same_words inventory-old inventory-new
check "the --out files hold detail (a note id) the streams do not" grep -q -F 1a2b3c4d-0000-4000-8000-000000000001 "$X/out/all-2-new.txt"
check "the --out file is 0600" test "$(stat -f '%Lp' "$X/out/all-2-new.txt")" = 600

# ---- one flipped byte; a missing file --------------------------------------------------
B=$(fresh flip)
F=$(find "$B/memory/owners" -name '*--*.md' | LC_ALL=C sort | head -1)
python3 -I -c 'import sys; p=sys.argv[1]; b=bytearray(open(p,"rb").read()); b[-2] ^= 1; open(p,"wb").write(bytes(b))' "$F"
M=$(fresh missing)
rm -f "$(find "$M/projects" -name '*--*.md' | LC_ALL=C sort | head -1)"
for v in old new; do
  run "flip-$v" "$v" snapshots --store "$B" --out "$X/out/flip-$v.txt"
  check "[$v] flipped byte: the home partition mismatched=1, matched=2; the project untouched" \
    test "$(field "flip-$v" "snapshots	1	" mismatched)/$(field "flip-$v" "snapshots	1	" matched)/$(field "flip-$v" "snapshots	2	" mismatched)" = "1/2/0"
  run "missing-$v" "$v" snapshots --store "$M" --out "$X/out/missing-$v.txt"
  check "[$v] missing file: the project partition missing=1, matched=1" \
    test "$(field "missing-$v" "snapshots	2	" missing)/$(field "missing-$v" "snapshots	2	" matched)/$(field "missing-$v" "snapshots	1	" missing)" = "1/1/0"
  check "[$v] flipped and missing: exit 0, stderr empty, no marker" \
    eval 'rc_is flip-'"$v"' 0 && rc_is missing-'"$v"' 0 && err_empty flip-'"$v"' && err_empty missing-'"$v"' && no_marker flip-'"$v"' missing-'"$v"''
done
check "flipped byte: old equals new" same_words flip-old flip-new
check "missing file: old equals new" same_words missing-old missing-new

# ---- the recorded root is never read ----------------------------------------------------
MK=$X/MARKER-e8-root
mkdir -p "$MK/notes"
for f in "$A"/memory/owners/*/notes/*.md "$A"/projects/*/memory/owners/*/notes/*.md; do
  printf 'a decoy with another revision\n' >"$MK/notes/$(basename "$f")"
done
RR=$(fresh root)
for s in "$RR"/state/memory-control/*.json; do
  python3 -I -c 'import json,sys; p,r=sys.argv[1:3]; s=json.load(open(p)); s["root"]=r; open(p,"w").write(json.dumps(s, indent=2)+"\n")' "$s" "$MK/"
done
for v in old new; do run "clean-snap-$v" "$v" snapshots --store "$A" --out "$X/out/clean-snap-$v.txt"; done
for v in old new; do
  run "root-open-$v" "$v" snapshots --store "$RR" --out "$X/out/root-open-$v.txt"
  check "[$v] recorded root, decoy folder readable: the counts equal the clean run's, no error" \
    eval 'same_words root-open-'"$v"' clean-snap-'"$v"' && rc_is root-open-'"$v"' 0 && err_empty root-open-'"$v"' && ! grep -q -E "error|unreadable=[1-9]" "$RUNS/root-open-'"$v"'.out"'
done
chmod 000 "$MK"
for v in old new; do
  run "root-000-$v" "$v" snapshots --store "$RR" --out "$X/out/root-000-$v.txt"
  check "[$v] recorded root, decoy folder chmod 000: the counts equal the clean run's, no error" \
    eval 'same_words root-000-'"$v"' clean-snap-'"$v"' && rc_is root-000-'"$v"' 0 && err_empty root-000-'"$v"''
done
chmod 700 "$MK"
# Under sandbox-exec denying file-read* on the decoy folder, with the kernel's
# sandbox reports streamed (`log show` after the fact does not hold them; the
# stream does): a calibration read by cat must show, an e8probe read must not.
SB=$X/deny-root.sb
printf '(version 1)\n(allow default)\n(deny file-read* (subpath "%s"))\n' "$MK" >"$SB"
LS_RAW=$X/sandbox-stream.raw
/usr/bin/log stream --style compact --predicate 'sender == "Sandbox"' >"$LS_RAW" 2>&1 &
LS_PID=$!
sleep 3
CAL_RC=0
/usr/bin/sandbox-exec -f "$SB" /bin/cat "$MK/notes/$(basename "$F")" >/dev/null 2>&1 || CAL_RC=$?
for v in old new; do
  rc=0
  /usr/bin/sandbox-exec -f "$SB" "$([ "$v" = old ] && echo "$OLD" || echo "$NEW")" snapshots --store "$RR" \
    --out "$X/out/root-sb-$v.txt" >"$RUNS/root-sb-$v.out" 2>"$RUNS/root-sb-$v.err" </dev/null || rc=$?
  echo "$rc" >"$RUNS/root-sb-$v.rc"
done
sleep 4
kill "$LS_PID" 2>/dev/null || true
wait "$LS_PID" 2>/dev/null || true
# Only the lines naming the decoy folder are kept; the raw stream (other
# processes' reports) is removed.
grep -F "$MK" "$LS_RAW" >"$X/sandbox-stream.decoy" || true
rm -f "$LS_RAW"
check "sandbox calibration: cat's read of the decoy folder was refused (rc $CAL_RC) and reported" \
  eval '[ "$CAL_RC" != 0 ] && grep -q -E " cat\([0-9]+\) deny\([0-9]+\) file-read" "$X/sandbox-stream.decoy"'
for v in old new; do
  check "[$v] recorded root under sandbox-exec: the counts equal the clean run's, no error" \
    eval 'same_words root-sb-'"$v"' clean-snap-'"$v"' && rc_is root-sb-'"$v"' 0 && err_empty root-sb-'"$v"''
done
# A report names the process by its executable's file name (cut to 16 bytes,
# MAXCOMLEN): `e8probe` when built in place, `e8probe-new` from a bins
# folder. So the names come from the two binaries under test.
probe_reads_reported() {
  local b n
  for b in "$OLD" "$NEW"; do
    n=$(basename -- "$b" | cut -c1-16)
    if grep -q -F -- " $n(" "$X/sandbox-stream.decoy"; then return 0; fi
  done
  return 1
}
check "recorded root under sandbox-exec: no read of the decoy folder reported for either probe, by its own process name ($(basename -- "$OLD"), $(basename -- "$NEW"))" \
  eval '! probe_reads_reported'

# ---- markers on neither stream: the panic path -----------------------------------------
PM=$(fresh panic)
mkdir -p "$X/out/panic"
for v in old new; do
  for c in db index fts-rw search snapshots inventory; do
    case "$c" in
      db) args=(db --db "$PM/state/openalpaca.db" --out "$X/out/panic/$c-$v.txt") ;;
      index) args=(index --cache "$PM/state/cache/memory-index-v1.sqlite" --out "$X/out/panic/$c-$v.txt") ;;
      fts-rw) cp -p "$CRO" "$X/out/panic/rw-$v.sqlite"; args=(fts-rw --cache-rw "$X/out/panic/rw-$v.sqlite" --cache-ro "$CRO") ;;
      search) args=(search record --cache "$PM/state/cache/memory-index-v1.sqlite" --record "$X/out/panic/rec-$v") ;;
      snapshots | inventory) args=("$c" --store "$PM" --out "$X/out/panic/$c-$v.txt") ;;
    esac
    (export E8_SELFTEST_PANIC="$c"; run "panic-$c-$v" "$v" "${args[@]}")
    check "[$v] E8_SELFTEST_PANIC=$c: \`$c<TAB>panic\`, exit 1, stderr only the fixed line, no marker" \
      eval 'has panic-'"$c"'-'"$v"' "'"$c"'	panic" && rc_is panic-'"$c"'-'"$v"' 1 && err_is panic-'"$c"'-'"$v"' "$PANIC_LINE" && no_marker panic-'"$c"'-'"$v"''
  done
  (export E8_SELFTEST_PANIC=nonsense; run "knob-$v" "$v" version)
  check "[$v] a knob of another value is refused before anything runs" \
    eval 'rc_is knob-'"$v"' 2 && err_is knob-'"$v"' "e8probe: refused-selftest-knob" && [ ! -s "$RUNS/knob-'"$v"'.out" ]'
done

# ---- un-checkpointed WAL rows -------------------------------------------------------------
WV=$(fresh wal)
for v in old new; do run "wal-before-$v" "$v" db --db "$WV/state/openalpaca.db" --out "$X/out/wal-before-$v.txt"; done
BEFORE=$(sed -n 's/^db\trows-total \([0-9]*\)$/\1/p' "$RUNS/wal-before-new.out")
# shellcheck disable=SC2016  # Python code
python3 -I -c '
import os, sqlite3, sys
c = sqlite3.connect(sys.argv[1], isolation_level=None)
c.execute("PRAGMA wal_autocheckpoint=0")
c.execute("BEGIN")
c.execute("INSERT INTO system_config (key, value, kind) VALUES (?, ?, ?)", ("e8.wal-row", "uncheckpointed", "string"))
c.execute("COMMIT")
os._exit(0)
' "$WV/state/openalpaca.db"
check "WAL: the commit left a non-empty -wal" test -s "$WV/state/openalpaca.db-wal"
# Every file of the store but the -shm, hashed before and after the reads of
# a -wal that holds frames: the database and the -wal byte-identical.
hashes "$WV" | grep -v -E -- '-shm$' >"$X/out/wal-reads.before" || true
for v in old new; do
  run "wal-after-$v" "$v" db --db "$WV/state/openalpaca.db" --out "$X/out/wal-after-$v.txt"
  check "[$v] WAL: rows-total is the pre-insert count plus 1 ($BEFORE + 1)" has "wal-after-$v" "db	rows-total $((BEFORE + 1))"
done
hashes "$WV" | grep -v -E -- '-shm$' >"$X/out/wal-reads.after" || true
check "WAL: the database and its -wal (frames in it) byte-identical across both variants' reads, only a -shm may change" \
  eval 'grep -q -E -- "-wal$" "$X/out/wal-reads.before" && cmp -s "$X/out/wal-reads.before" "$X/out/wal-reads.after"'
IMM=0
while IFS= read -r t; do
  IMM=$((IMM + $(sq "file:$WV/state/openalpaca.db?immutable=1" "SELECT count(*) FROM \"$t\";")))
done < <(sq "file:$WV/state/openalpaca.db?immutable=1" "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name;")
check "WAL control: sqlite3 with immutable=1 counts one fewer ($IMM against $((BEFORE + 1)))" test "$IMM" = "$BEFORE"
check "WAL: the -wal is still there after the ro opens (nothing checkpointed it)" test -s "$WV/state/openalpaca.db-wal"

# ---- FTS corruption -------------------------------------------------------------------------
for v in old new; do
  cp -p "$CRO" "$X/out/corrupt-$v.sqlite"
  sq "$X/out/corrupt-$v.sqlite" 'UPDATE fts_word_data SET block = zeroblob(length(block)) WHERE id = (SELECT max(id) FROM fts_word_data);'
  run "corrupt-rw-$v" "$v" fts-rw --cache-rw "$X/out/corrupt-$v.sqlite" --cache-ro "$CRO"
  check "[$v] FTS corruption: fts-rw reads fts_word corrupt, the other two ok" \
    eval 'has corrupt-rw-'"$v"' "fts-rw	fts_word corrupt" && has corrupt-rw-'"$v"' "fts-rw	fts_tri_head ok" && has corrupt-rw-'"$v"' "fts-rw	fts_tri ok" && err_empty corrupt-rw-'"$v"''
  cp -p "$X/out/corrupt-$v.sqlite" "$X/out/corrupt-ro-$v.sqlite"
  run "corrupt-ix-$v" "$v" index --cache "$X/out/corrupt-ro-$v.sqlite" --out "$X/out/corrupt-ix-$v.txt"
  check "[$v] FTS corruption: index reads integrity not-ok" has "corrupt-ix-$v" "index	integrity not-ok"
done

# ---- the ro open changes nothing ---------------------------------------------------------------
for v in old new; do
  for c in db index search snapshots; do
    S=$(fresh "ro-$c-$v")
    hashes "$S" >"$X/out/ro-$c-$v.before"
    case "$c" in
      db) run "ro-$c-$v" "$v" db --db "$S/state/openalpaca.db" --out "$X/out/ro-$c-$v.txt" ;;
      index) run "ro-$c-$v" "$v" index --cache "$S/state/cache/memory-index-v1.sqlite" --out "$X/out/ro-$c-$v.txt" ;;
      search) run "ro-$c-$v" "$v" search record --cache "$S/state/cache/memory-index-v1.sqlite" --record "$X/out/ro-$c-$v.rec" ;;
      snapshots) run "ro-$c-$v" "$v" snapshots --store "$S" --out "$X/out/ro-$c-$v.txt" ;;
    esac
    hashes "$S" >"$X/out/ro-$c-$v.after"
    # Lines that changed or appeared, other than a -shm's and a zero-length
    # -wal that did not exist before (the WAL reader's sidecar, above).
    LC_ALL=C comm -13 "$X/out/ro-$c-$v.before" "$X/out/ro-$c-$v.after" | grep -v -E -- '-shm$' |
      awk -v before="$X/out/ro-$c-$v.before" 'BEGIN { while ((getline l < before) > 0) { split(l, f, "  "); had[f[2]] = 1 } }
        { split($0, f, "  ") } f[1] == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" && f[2] ~ /-wal$/ && !(f[2] in had) { next } { print }' \
      >"$X/out/ro-$c-$v.changed" || true
    LC_ALL=C comm -23 "$X/out/ro-$c-$v.before" "$X/out/ro-$c-$v.after" | grep -v -E -- '-shm$' >"$X/out/ro-$c-$v.gone" || true
    check "[$v] $c: exit 0; every file of the input unchanged but a -shm and a new zero-length -wal, none removed" \
      eval 'rc_is ro-'"$c"'-'"$v"' 0 && [ ! -s "$X/out/ro-'"$c"'-'"$v"'.changed" ] && [ ! -s "$X/out/ro-'"$c"'-'"$v"'.gone" ]'
  done
done

# ---- projects as the daemon's list() reads them; .history ------------------------------------------
PJ=$(fresh proj-shared)
PD=$(find "$PJ/projects" -mindepth 1 -maxdepth 1 -type d | head -n 1)
cp -Rp "$PD" "$PD-copied"
mkdir -p "$PJ/projects/.registering"
cp -p "$PD/project.json" "$PJ/projects/.registering/project.json"
PN=$(fresh proj-no-created-at)
python3 -I -c 'import json,sys; p=sys.argv[1]; s=json.load(open(p)); del s["created_at"]; open(p,"w").write(json.dumps(s))' \
  "$(find "$PN/projects" -name project.json)"
PR=$(fresh proj-relative-root)
python3 -I -c 'import json,sys; p=sys.argv[1]; s=json.load(open(p)); s["root"]="relative/root"; open(p,"w").write(json.dumps(s))' \
  "$(find "$PR/projects" -name project.json)"
HL=$(fresh history-link)
mkdir -p "$X/history-elsewhere"
ln -s "$X/history-elsewhere" "$(dirname "$(find "$HL/memory/owners" -type d -name notes)")/.history"
HF=$(fresh history-file)
: >"$(dirname "$(find "$HF/memory/owners" -type d -name notes)")/.history"
for v in old new; do
  run "pshared-$v" "$v" snapshots --store "$PJ" --out "$X/out/pshared-$v.txt"
  run "pshared-inv-$v" "$v" inventory --store "$PJ" --out "$X/out/pshared-inv-$v.txt"
  check "[$v] two folders sharing a project id: both left out, the project partition folder=orphan, its 2 notes unlocated, not missing; the hidden folder passed over" \
    eval 'test "$(field pshared-'"$v"' "snapshots	2	" folder)/$(field pshared-'"$v"' "snapshots	2	" unlocated)/$(field pshared-'"$v"' "snapshots	2	" missing)/$(field pshared-'"$v"' "snapshots	1	" matched)" = "orphan/2/0/3" && has pshared-inv-'"$v"' "inventory	projects folders=2 readable=0 unreadable=0 shared-id=2 hidden=1 link-skipped=0 other=0" && has pshared-inv-'"$v"' "inventory	owner-folders keyed=1 unkeyed=2 link-skipped=0 other=0"'
  for c in no-created-at relative-root; do
    S=$X/proj-$c-$MARK/store
    run "p$c-$v" "$v" snapshots --store "$S" --out "$X/out/p$c-$v.txt"
    run "p$c-inv-$v" "$v" inventory --store "$S" --out "$X/out/p$c-inv-$v.txt"
    check "[$v] a project.json with $c: unreadable, as the daemon reads it; its partition folder=orphan" \
      eval 'test "$(field p'"$c"'-'"$v"' "snapshots	2	" folder)/$(field p'"$c"'-'"$v"' "snapshots	2	" unlocated)" = "orphan/2" && has p'"$c"'-inv-'"$v"' "inventory	projects folders=1 readable=0 unreadable=1 shared-id=0 hidden=0 link-skipped=0 other=0"'
  done
  for h in link file; do
    S=$X/history-$h-$MARK/store
    run "hist-$h-$v" "$v" snapshots --store "$S" --out "$X/out/hist-$h-$v.txt"
    check "[$v] a $h at the home owner folder's .history: notes-dir=link-skipped, its 3 notes unlocated; the project partition untouched" \
      eval 'test "$(field hist-'"$h"'-'"$v"' "snapshots	1	" notes-dir)/$(field hist-'"$h"'-'"$v"' "snapshots	1	" unlocated)/$(field hist-'"$h"'-'"$v"' "snapshots	1	" matched)/$(field hist-'"$h"'-'"$v"' "snapshots	2	" matched)" = "link-skipped/3/0/2" && rc_is hist-'"$h"'-'"$v"' 0'
  done
  check "[$v] projects and .history: no marker, inside the alphabet" \
    eval 'no_marker pshared-'"$v"' pshared-inv-'"$v"' hist-link-'"$v"' hist-file-'"$v"' && in_alphabet pshared-inv-'"$v"' && in_alphabet hist-link-'"$v"''
done
check "projects and .history: old equals new" \
  eval 'same_words pshared-old pshared-new && same_words pshared-inv-old pshared-inv-new && same_words hist-link-old hist-link-new'

# ---- refusals before any open --------------------------------------------------------------------
refused() {   # name expected-line — exit 2, exactly that one stderr line, nothing on stdout
  rc_is "$1" 2 && err_is "$1" "$2" && [ ! -s "$RUNS/$1.out" ]
}
DOT=$X/refuse/.openalpaca
mkdir -p "$X/refuse"
cp -Rp "$ORIG" "$DOT"
ln -s "$DOT" "$X/refuse/link-to-store"
LIVE_D=$(fresh live-discovery)
: >"$LIVE_D/state/discovery.json"
LIVE_L=$(fresh live-lock)
: >"$LIVE_L/state/openalpacad.lock"
for v in old new; do
  run "ref-dot-$v" "$v" snapshots --store "$DOT" --out "$X/out/ref-dot-$v.txt"
  check "[$v] a .openalpaca component: refused" refused "ref-dot-$v" "e8probe: refused-openalpaca-path"
  run "ref-dotcase-$v" "$v" snapshots --store "$X/refuse/.OpenAlpaca" --out "$X/out/ref-dotcase-$v.txt"
  check "[$v] a .OpenAlpaca component (another letter case, as given): refused" refused "ref-dotcase-$v" "e8probe: refused-openalpaca-path"
  run "ref-dotdb-$v" "$v" db --db "$DOT/state/openalpaca.db" --out "$X/out/ref-dotdb-$v.txt"
  check "[$v] a .openalpaca component (db): refused, no -shm made" \
    eval 'refused ref-dotdb-'"$v"' "e8probe: refused-openalpaca-path" && [ ! -e "$DOT/state/openalpaca.db-shm" ]'
  run "ref-link-$v" "$v" all --store "$X/refuse/link-to-store" --cache-ro "$CRO" --cache-rw "$X/cache-rw-$v/memory-index-v1.sqlite" \
    --record "$X/out/ref-link-$v.rec" --out "$X/out/ref-link-$v.txt"
  check "[$v] a .openalpaca component reached through a link: refused" refused "ref-link-$v" "e8probe: refused-openalpaca-path"
  run "ref-disc-$v" "$v" all --store "$LIVE_D" --cache-ro "$CRO" --cache-rw "$X/cache-rw-$v/memory-index-v1.sqlite" \
    --record "$X/out/ref-disc-$v.rec" --out "$X/out/ref-disc-$v.txt"
  check "[$v] a store holding state/discovery.json: refused, no -shm made, no --out written" \
    eval 'refused ref-disc-'"$v"' "e8probe: refused-store-has-discovery-json" && [ ! -e "$LIVE_D/state/openalpaca.db-shm" ] && [ ! -e "$X/out/ref-disc-'"$v"'.txt" ]'
  run "ref-discdb-$v" "$v" db --db "$LIVE_D/state/openalpaca.db" --out "$X/out/ref-discdb-$v.txt"
  check "[$v] the database of a store holding state/discovery.json: refused" \
    refused "ref-discdb-$v" "e8probe: refused-store-has-discovery-json"
  run "ref-lock-$v" "$v" snapshots --store "$LIVE_L" --out "$X/out/ref-lock-$v.txt"
  check "[$v] a store holding state/openalpacad.lock: refused, no -shm made" \
    eval 'refused ref-lock-'"$v"' "e8probe: refused-store-has-daemon-lock" && [ ! -e "$LIVE_L/state/openalpaca.db-shm" ]'
  ST=$(fresh "state-rw-$v")
  run "ref-rwstate-$v" "$v" fts-rw --cache-rw "$ST/state/cache/memory-index-v1.sqlite" --cache-ro "$CRO"
  check "[$v] --cache-rw under a state/ folder: refused, no -shm made" \
    eval 'refused ref-rwstate-'"$v"' "e8probe: refused-cache-rw-under-state" && [ ! -e "$ST/state/cache/memory-index-v1.sqlite-shm" ]'
  SAME=$(fresh "same-$v")/state/cache/memory-index-v1.sqlite
  mkdir -p "$X/same-$v"
  cp -p "$SAME" "$X/same-$v/memory-index-v1.sqlite"
  SAME=$X/same-$v/memory-index-v1.sqlite
  run "ref-same-$v" "$v" fts-rw --cache-rw "$SAME" --cache-ro "$SAME"
  check "[$v] --cache-rw the same file as --cache-ro: refused, no -shm made" \
    eval 'refused ref-same-'"$v"' "e8probe: refused-cache-rw-is-cache-ro" && [ ! -e "$SAME-shm" ]'
  ln -f "$CRO" "$X/out/hardlink-$v.sqlite"
  run "ref-hard-$v" "$v" fts-rw --cache-rw "$X/out/hardlink-$v.sqlite" --cache-ro "$CRO"
  check "[$v] --cache-rw a hard link of --cache-ro: refused" refused "ref-hard-$v" "e8probe: refused-cache-rw-is-cache-ro"
  : >"$X/out/exists-$v.txt"
  run "ref-out-$v" "$v" snapshots --store "$A" --out "$X/out/exists-$v.txt"
  check "[$v] an --out that exists: refused" refused "ref-out-$v" "e8probe: refused-out-exists"
  run "ref-outin-$v" "$v" snapshots --store "$A" --out "$A/probe-out-$v.txt"
  check "[$v] an --out inside --store: refused, nothing written there" \
    eval 'refused ref-outin-'"$v"' "e8probe: refused-output-inside-store" && [ ! -e "$A/probe-out-'"$v"'.txt" ]'
  run "ref-rec-$v" "$v" search record --cache "$CRO" --record "$X/out/search.record"
  check "[$v] search record over an existing record: refused" refused "ref-rec-$v" "e8probe: refused-record-exists"
  run "usage-$v" "$v" db --db "$A/state/openalpaca.db"
  check "[$v] a missing required flag: usage, exit 2" eval 'rc_is usage-'"$v"' 2 && [ ! -s "$RUNS/usage-'"$v"'.out" ]'
  run "usage2-$v" "$v" snapshots --store "$A" --out "$X/out/u2-$v.txt" --frobnicate x
  check "[$v] an unknown flag: usage, exit 2, no --out written" \
    eval 'rc_is usage2-'"$v"' 2 && [ ! -e "$X/out/u2-'"$v"'.txt" ]'
done
# A store whose state/ is a link to a full state folder elsewhere; one whose
# database is a link to a copy of it; a cache given through a link.
LS=$(fresh linked-state)
mkdir -p "$X/linked-state-target"
mv "$LS/state" "$X/linked-state-target/state"
ln -s "$X/linked-state-target/state" "$LS/state"
LD=$(fresh linked-db)
mkdir -p "$X/linked-db-target"
mv "$LD/state/openalpaca.db" "$X/linked-db-target/openalpaca.db"
ln -s "$X/linked-db-target/openalpaca.db" "$LD/state/openalpaca.db"
ln -s "$CRO" "$X/out/cache-link.sqlite"
for v in old new; do
  run "ref-lstate-$v" "$v" all --store "$LS" --cache-ro "$CRO" --cache-rw "$X/cache-rw-$v/memory-index-v1.sqlite" \
    --record "$X/out/ref-lstate-$v.rec" --out "$X/out/ref-lstate-$v.txt"
  check "[$v] all on a store whose state/ is a link: refused, no --out written" \
    eval 'refused ref-lstate-'"$v"' "e8probe: refused-state-is-a-link" && [ ! -e "$X/out/ref-lstate-'"$v"'.txt" ]'
  run "ref-lstate-snap-$v" "$v" snapshots --store "$LS" --out "$X/out/ref-lstate-snap-$v.txt"
  check "[$v] snapshots on a store whose state/ is a link: refused" refused "ref-lstate-snap-$v" "e8probe: refused-state-is-a-link"
  run "ref-lstate-db-$v" "$v" db --db "$LS/state/openalpaca.db" --out "$X/out/ref-lstate-db-$v.txt"
  check "[$v] db given through a linked state/ folder: refused" refused "ref-lstate-db-$v" "e8probe: refused-path-not-canonical"
  run "ref-ldb-$v" "$v" all --store "$LD" --cache-ro "$CRO" --cache-rw "$X/cache-rw-$v/memory-index-v1.sqlite" \
    --record "$X/out/ref-ldb-$v.rec" --out "$X/out/ref-ldb-$v.txt"
  check "[$v] all on a store whose openalpaca.db is a link: refused" refused "ref-ldb-$v" "e8probe: refused-path-not-canonical"
  run "ref-lcache-$v" "$v" index --cache "$X/out/cache-link.sqlite" --out "$X/out/ref-lcache-$v.txt"
  check "[$v] index given a link to a cache: refused" refused "ref-lcache-$v" "e8probe: refused-path-not-canonical"
  # Hard links of the store's own index and database, outside any state/.
  HS=$(fresh "hard-$v")
  mkdir -p "$X/hard-$v"
  ln -f "$HS/state/cache/memory-index-v1.sqlite" "$X/hard-$v/index.sqlite"
  ln -f "$HS/state/openalpaca.db" "$X/hard-$v/db.sqlite"
  run "ref-hidx-$v" "$v" all --store "$HS" --cache-ro "$CRO" --cache-rw "$X/hard-$v/index.sqlite" \
    --record "$X/out/ref-hidx-$v.rec" --out "$X/out/ref-hidx-$v.txt"
  check "[$v] all with --cache-rw a hard link of the store's own index: refused" refused "ref-hidx-$v" "e8probe: refused-cache-rw-is-an-input"
  run "ref-hdb-$v" "$v" all --store "$HS" --cache-ro "$CRO" --cache-rw "$X/hard-$v/db.sqlite" \
    --record "$X/out/ref-hdb-$v.rec" --out "$X/out/ref-hdb-$v.txt"
  check "[$v] all with --cache-rw a hard link of the store's own database: refused" refused "ref-hdb-$v" "e8probe: refused-cache-rw-is-an-input"
  run "ref-hnl-$v" "$v" fts-rw --cache-rw "$X/hard-$v/index.sqlite" --cache-ro "$CRO"
  check "[$v] fts-rw with --cache-rw of another hard link (no --store to compare): refused" refused "ref-hnl-$v" "e8probe: refused-cache-rw-has-another-link"
done
check "no -shm or -wal made through the links, in the targets or beside the hard links" \
  test -z "$(find "$X/linked-state-target" "$X/linked-db-target" "$LS" "$LD" "$X"/hard-old "$X"/hard-new "$X"/hard-old-"$MARK" "$X"/hard-new-"$MARK" \( -name '*-shm' -o -name '*-wal' \) -print)"
check "no -shm or -wal beside the refused stores' databases and caches" \
  test -z "$(find "$DOT" "$LIVE_D" "$LIVE_L" "$X"/state-rw-*-"$MARK" "$X"/same-* \( -name '*-shm' -o -name '*-wal' \) -print)"

echo "probe-test: $PASS passed, $FAILS failed"
[ "$FAILS" = 0 ]
