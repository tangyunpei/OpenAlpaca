# E8-PROBE — read-only SQLite and sha2 checks of a store copy, and E8 as one command

The plan of record is [`../../programme-plan.md`](../../programme-plan.md): §4.5 *E8* (the owner's rehearsal on a copy of the real store) and *E8-PROBE* (this tool). The E6 tools beside it, [`../e6/`](../e6/README.md), are its form model.

**Status: rev 1, written 2026-10-29 at `0286a80f` and landed on `19b716ac`, re-read against that tip (programme-plan §4.5 E8-PROBE; a Chunk 25 item of its own). It moves no workspace version and touches no product code: the crate has its own `[workspace]`, so the workspace never sees it, and the workspace `Cargo.lock` is byte-identical. It is run by the owner, at E8, before the first start of a bumped build on the real store (§5 item 4 follows it). Revision log at the end.**

E8 is "the only check of the owner's existing partitions, snapshots and database under SQLite 3.53.2 and sha2 0.11 on schema 4 / cache 6" (§4.5 E8). Its step 7 asks for a rebuilt current-source probe in two variants that differ only in rusqlite and sha2, which, on a copy of the store and writing nothing to it:
- runs `PRAGMA integrity_check` and `PRAGMA foreign_key_check` read-only;
- runs FTS5's `integrity-check` `INSERT` only on a second, read-write scratch copy of the cache, on all three tables;
- counts the rows step 3 counted, and compares;
- runs the same search over the index copy under both variants, with the same ids as the answer;
- re-derives, for every control snapshot, each note file's revision and the partition key with both variants' `sha256_hex`, and prints mismatch counts only.

Step 8 reopens the copy with the old variant. The archived `sqlite-probe` is **not** this probe (R12): it had no `src/database/`, registries 43/44/45, cache 3, two FTS tables, and printed failures instead of returning them.

**The tip's index is at schema 7; the owner's is at 6.** `memory_index::SCHEMA_VERSION` went 6 → 7 at the sweep's S12 (`252e9c2e`; no table changed: `SCHEMA_SQL` and the rank weights are the same at the branch point, `0286a80f` and `19b716ac`). The owner's index was written at 6 by the running pre-wave build, and E8 runs before a bumped build first starts on the real store (CLAUDE.md's *Before this build's first start* line; §4.5 E8 *Place*), so the copy's index reads 6, which is what the probe's `index` check expects (`schema-expected` in `--out`) and what corpus B holds. A bumped build's first start rebuilds the index at 7, off the request path; E8 is over by then.

## The lead's readings (each the lead's, reversible)

1. **One toolchain** (runbook §7 item 8). Both variants build under the tip's `rust-toolchain.toml` (1.99.1). Both locks start from the tip's `Cargo.lock` as it stood at `0286a80f`, trimmed to the probe's graph (*The two locks* says what `19b716ac` has moved since), and `old/` moves back only the rusqlite and sha2 subtrees to the base's versions. The plan's "built from the branch point" is read as "the branch point's rusqlite and sha2", not "the branch point's compiler and lock": built whole from `bc733970`'s lock and its 1.93.0, the variants would also differ in cc 1.4.4 ↔ 1.8.0, serde_json 1.0.151 ↔ 1.0.154, libc, smallvec and more, which breaks "differing only in". rusqlite 0.38.0 and sha2 0.10.9 already built and passed the full suite on 1.99.1: P04's lock `e65b4eaa` held exactly them beside cc 1.8.0.
2. **`rehearsal.sh` is built** (runbook §7 item 4; the owner chose the one-command form for E6). It runs E8's steps 2–8 as one owner-run command in `../e6/consent-point.sh`'s form, given the owner's copy and the binaries folder. Its paste block carries only commits, binary hashes, versions, words, counts and partition keys, filtered to `[A-Za-z0-9 ._:=(),+-]` and tab under `LC_ALL=C`. Its test, `rehearsal-test.sh`, runs on synthetic stores only. Step 1 stays the owner's typed command (stopping the real daemon, the `cp -Rp`, checking that neither the copy nor its `state/` is a link, removing the copy's discovery file and lock), printed by `rehearsal.sh --help`. The script refuses, before anything else, a `--copy` with a `.openalpaca` component in any letter case, one that is a link or holds one where a step would follow it, and one whose `state/discovery.json` or `state/openalpacad.lock` exists.
3. **`new/` pins exactly what the tip's lock holds** for rusqlite and sha2: 0.40.2 and 0.11.1 at `0286a80f`, and still at `19b716ac`, after the sweep's commits so far (re-checked when this landed). The re-pin rule below says what happens when the FRESHNESS-SWEEP moves either; the lead re-checks it after the sweep's closing pass.
4. **The shared crates stay at `0286a80f`'s until a re-pin** (on landing at `19b716ac`). The four that the sweep's `2ef1197b` has moved in the workspace lock since — cc, find-msvc-tools, libc, smallvec — keep `0286a80f`'s versions in both locks, the same in each, so the variants still differ in the two subtrees alone. The pins reach only rusqlite and sha2, but a re-pin derives both locks again from `TIP`'s, so it brings these four, and any other shared crate `TIP` has moved, to `TIP`'s versions, still the same in each (*The two locks*, *Re-pin*).

The lead also added the `verdict` line to the rehearsal's block. It reads the plan's gate from the block's own lines, and every line it reads stays in the block for the lead to judge again. It is informational and reversible.

## Files

| File | Contents |
|---|---|
| `src/main.rs` | The one source both variants build. The header lists the subcommands. |
| `old/Cargo.toml`, `new/Cargo.toml` | `[package] name = "e8probe"`, `publish = false`, an empty `[workspace]`, `[[bin]] path = "../src/main.rs"`. Dependencies: `rusqlite` (`["bundled"]`, the workspace's only feature), `sha2`, and `serde_json = "=1.0.154"`. `diff old/Cargo.toml new/Cargo.toml` shows two lines: `rusqlite =0.38.0` ↔ `=0.40.2` and `sha2 =0.10.9` ↔ `=0.11.1`. |
| `old/Cargo.lock`, `new/Cargo.lock` | Committed, so every build is `--locked` (*The two locks*). |
| `probe-test.sh` | The probe's test on synthetic stores (*How it is tested*). Its `--corpus-only <dir>` writes corpus B and exits. |
| `rehearsal.sh` | E8 steps 2–8 as one owner-run command (*One command*). |
| `rehearsal-test.sh` | `rehearsal.sh`'s test on synthetic stores, with the real binaries and with fakes. |

`git check-ignore -q` exits 1 for each file: `README.md` is whitelisted by `.gitignore`'s `!README.md`, and the others are not `*.md`.

## Build

```sh
R=$(git rev-parse --show-toplevel)          # the tree at TIP
E=$R/tasks/deps-upgrade/tools/e8probe
X=/path/to/a/scratch/dir                    # outside the repository; removed whole when done
mkdir -p "$X/bin"
TC=$(sed -n 's/^channel *= *"\(.*\)"$/\1/p' "$R/rust-toolchain.toml")   # never a literal (A-14)
for v in old new; do
  CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0 CARGO_HTTP_USER_AGENT=openalpaca-deps-check \
    CARGO_TARGET_DIR="$X/target/$v" CARGO_BUILD_BUILD_DIR="$X/target/$v" \
    cargo "+$TC" build --release --locked --manifest-path "$E/$v/Cargo.toml"
  cp "$X/target/$v/release/e8probe" "$X/bin/e8probe-$v"
done
# the known answer, each variant (cfg(test)):
for v in old new; do CARGO_TARGET_DIR="$X/target/$v" CARGO_BUILD_BUILD_DIR="$X/target/$v" \
  cargo "+$TC" test --locked --manifest-path "$E/$v/Cargo.toml"; done
"$X/bin/e8probe-old" version    # e8probe<TAB>sqlite 3.51.1, e8probe<TAB>sha2-known-answer ok
"$X/bin/e8probe-new" version    # e8probe<TAB>sqlite 3.53.2, ...
```

This is the E6 README's `c()` form. `CARGO_BUILD_BUILD_DIR` is set to the target dir (MF-5): with `CARGO_TARGET_DIR` alone, the intermediates would land in `~/.cargo/build/<hash>/` (M63), where removing `$X` never reaches. A cold release build takes about 12 s per variant here; most of that is the bundled SQLite's C.

**What the owner's E8 needs from the lead.** One folder (`$EG/bins/` in the runbook) holding:
- `e8probe-old` and `e8probe-new`, built as above at `TIP`, after the re-pin check;
- `openalpaca-prewave`, E7's pre-wave CLI (`bc733970`, release);
- `openalpaca-tip`, the CLI of `TIP` (release);
- `SHA256SUMS`, `shasum -a 256` over at least those four. E7's `openalpacad-prewave` may stay listed beside them; `rehearsal.sh` checks every entry.

E7's own `SHA256SUMS` (runbook §E7) lists `openalpacad-prewave openalpaca-prewave openalpaca-tip` only. Once the two probes are in that folder, regenerate it over all five, or `rehearsal.sh` refuses the folder (`bins-checksums failed`). The same folder serves `rehearsal-test.sh --bins`.

## The two locks

Both locks start from the tip's `Cargo.lock` at `0286a80f`. That lock was copied beside `new/Cargo.toml`, and one build without `--locked` trimmed it to the probe's graph; cargo keeps locked versions. That lock was then copied beside `old/Cargo.toml`, and `cargo update --workspace` moved only what the two pins reach. It resolved to the base's versions with no `--precise` needed: libsqlite3-sys `^0.36.0` took 0.36.0, and crypto-common 0.1.7 pins generic-array `=0.14.7`. The committed locks hold:

| Subtree | `old/` (= `bc733970` and `e65b4eaa`) | `new/` (= `0286a80f` and `19b716ac`) |
|---|---|---|
| rusqlite | rusqlite 0.38.0, libsqlite3-sys 0.36.0 (bundled SQLite 3.51.1), hashlink 0.11.1, hashbrown 0.16.1 | rusqlite 0.40.2, libsqlite3-sys 0.38.2 (3.53.2), hashlink 0.12.2, hashbrown 0.16.1 and 0.17.1 |
| sha2 | sha2 0.10.9, digest 0.10.7, block-buffer 0.10.4, crypto-common 0.1.7, generic-array 0.14.7, version_check 0.9.5, cpufeatures 0.2.17 | sha2 0.11.1, digest 0.11.3, block-buffer 0.12.1, crypto-common 0.2.2, hybrid-array 0.4.15, const-oid 0.10.2, cpufeatures 0.3.1 |

Everything else is the same in both, at `0286a80f`'s versions: serde_json 1.0.154, cc 1.8.0, typenum 1.20.1 and the rest. Since then the sweep's lock-only commit `2ef1197b` has moved four of them in the workspace lock: cc 1.10.0, find-msvc-tools 0.1.15, libc 0.2.191 and smallvec 1.16.4 at `19b716ac`. Both locks keep `0286a80f`'s, the same in each, so the variants still differ in the two subtrees alone, until a re-pin, which derives both locks again from `TIP`'s and so takes `TIP`'s versions for these four too; the pins reach only rusqlite and sha2 (reading 4; *Re-pin*). cmov and ctutils (P11's waiver) are not in the probe's graph: they enter the workspace through features the probe does not enable.

The checks, rerun whenever a lock moves:

```sh
cd "$R"
python3 -I tasks/deps-upgrade/tools/gbase/lockdiff.py \
  --expected rusqlite-subtree=rusqlite,libsqlite3-sys,hashlink,hashbrown \
  --expected sha2-subtree=sha2,digest,block-buffer,crypto-common,generic-array,hybrid-array,cpufeatures,const-oid,version_check \
  "$E/old/Cargo.lock" "$E/new/Cargo.lock"            # unexpected: 0; exit 0
pkgs() { awk '/^name = /{n=$3} /^version = /{print n, $3}' "$1" | tr -d '"' | LC_ALL=C sort -u; }
{ pkgs <(git show bc733970:Cargo.lock); pkgs <(git show 0286a80f:Cargo.lock); } | LC_ALL=C sort -u > "$X/known"   # after a re-pin, TIP's in 0286a80f's place
for v in old new; do LC_ALL=C comm -23 <(pkgs "$E/$v/Cargo.lock") "$X/known"; done   # prints `e8probe 0.1.0` twice, nothing else
git diff --quiet HEAD -- Cargo.lock && echo workspace-lock-unchanged
```

Every `(name, version)` in either lock is already in the base's lock or in `0286a80f`'s, the one both were trimmed from. No new release enters, so the freshness gate has nothing to read.

**Re-pin: `new/` is the build the owner will start.** E8 runs after the FRESHNESS-SWEEP, and the bundled SQLite's way out is the sweep: rusqlite 0.41 with libsqlite3-sys 0.39.1, eligible from 2026-11-01T05:20:35Z and taken by the sweep's closing pass as its own item if eligible then (programme-plan §3.2, the FRESHNESS-SWEEP's named blockers). That is the move this rule is for. The rule:
- `new/Cargo.toml` pins exactly the `rusqlite` and `sha2` that `TIP`'s `Cargo.lock` gives the workspace's own crates on the day. List every locked version with `awk '/^name = "(rusqlite|sha2)"$/{n=$3} /^version = /{if (n) print n, $3; n=""}' Cargo.lock`, and the ones `openalpaca_storage` takes with `awk '/^\[\[package\]\]$/{p=0} /^name = "openalpaca_storage"$/{p=1} p && /^ "(rusqlite|sha2)( [0-9.]+)?",?$/' Cargo.lock`. At `0286a80f`, and again at `19b716ac`, that is rusqlite 0.40.2, the lock's only one, and sha2 0.11.1; the lock's sha2 0.10.9 is lopdf's (through pdf-extract), tauri-codegen's and wry's (the shell's).
- If the sweep moved either, that line takes the lock's version. The tool is rebuilt once without `--locked`, so that `new/Cargo.lock` follows (from `TIP`'s lock, trimmed as above), and that build is the one run.
- `old/` keeps its pins at the base's 0.38.0 and 0.10.9, and its lock is derived again from the new `new/` lock, as above, so the two still differ in the two subtrees alone.
- The `comm` check, reading `TIP`'s lock in `0286a80f`'s place, then proves that every version is the base's or `TIP`'s. A probe on any other version answers nothing about the wave.
- The version checks then follow `new/`'s new versions: the two expected `sqlite <v>` checks (`probe-test.sh`'s `[new] sqlite …` and `rehearsal-test.sh`'s `[real] bins checked …`), the header of `src/main.rs`, and this README's mentions of `new/`'s versions (the *Build* comments, *The two locks*' table, *Proving read-only* and the first case of *How it is tested*; the plan's words quoted at the top stay the plan's). Then both tests run again, as *How it is tested* runs them.

## What it prints

Each stdout line is `<check>\t<rest>`. The rest is fixed words, counts, the SQLite version and, for snapshots and the inventory, partition keys (`home-<32 hex>`, `proj-<32 hex>-<16 hex>`), shape-checked before they are printed (`invalid-key` otherwise). The checks:
- `db`: `sqlite <v>`, `integrity ok|not-ok`, `fk <n>`, `schema-version <n>`, `schema-set 1,2,3,4|other`, `master tables= indexes= triggers= views=`, `addr-index expected|different|absent` (004's text), `tables <n>`, `rows-total <n>`, `in-project rows= roots=`, `extraction error= uploaded= processing=`, and with `--step3` `step3-counts equal|different`.
- `index`: `sqlite <v>`, `integrity ok|not-ok`, `schema <n>|other|absent`, `state ready|rebuilding|other|absent`, `fts-tables <n>`, `rows note= chunk= note_alias= note_source= partition=`.
- `fts-rw`: `<table> ok|corrupt|error|absent` for each of the three tables.
- `search`: `recorded queries <n> with-results <n> error <n>`. When comparing, `query-list equal|different`, then `<i>\tequal|different|error` (under `all`, only the non-equal ones), then `queries <n> with-results <n> equal <n> different <n> error <n>`. The with-results count shows the comparison was not vacuous.
- `snapshots`: one line per snapshot file, `<i>\t<key>\t<home|project|other>\tschema=\tkey=equal|different|absent\tfolder=home|project|orphan\tnotes-dir=present|absent|link-skipped` (`link-skipped`: a link or a file among the partition's directories, `notes` and `.history` included, which refuses the partition in the daemon, so its notes are not read), then the counts: `notes`, `matched`, `mismatched`, `mismatched-pending` (the file is the snapshot's `pending.to_rev`), `missing` (the folder was listed and the file is not in it), `unlocated` (the copy gives no folder: an orphan snapshot, such as an earlier build's project snapshot with no `project_id`, R22, or one whose id no `projects/*/project.json` records, or a notes folder reached through a link; neither the daemon nor the probe reads those files), `duplicate`, `oversize`, `unreadable`, `unrecorded`, `unnamed`, `forgotten-files`, `link-skipped`, `tombstones` and `pending`. A `total` line sums them. A file that does not parse is `<i>\t<key>\tunparsable`.
- `inventory`: `projects folders= readable= unreadable= shared-id= hidden= link-skipped= other=`, by `store/projects.rs`'s `list`: `readable` is what `list` returns; `unreadable` a folder whose `project.json` it does not read; `shared-id` the folders that carry one id between two or more, which `list` leaves out (all of them); `hidden` the dot-prefixed names it passes over. Then `owner-folders keyed= unkeyed= link-skipped= other=` (a folder `list` leaves out keys none of its owner folders), one `snapshot\t<i>\t<key>\t<scope>\tnotes=\ttombstones=\tproject-id=` per snapshot, one `partition\t<key>\tfolder=yes|no\tsnapshot=yes|no\tnote-files=` per partition the folders or the snapshots name, and `partitions total= both= folder-only= snapshot-only=`.
- A check that could not run prints `<check>\terror`, and a panic prints `<check>\tpanic`. The error's class (the step and SQLite's code, never its message) goes to `--out`.

The `--out` file (created new, 0600) holds the detail: table names and counts, `sqlite_master` entries as name and sha256 of their SQL, integrity rows, each note id with its word, project folder names, ids and recorded roots. The `--record` file holds an index, `ok` or `error`, a count and a sha256 per query: no id and no term. Both stay under the caller's scratch root and are never pasted.

Exit 0 means every check ran: `not-ok`, `different` or `corrupt` is a result. Exit 1 means a check printed `error` or `panic`. Exit 2 is a usage error or a refusal, before anything is opened. Exit 101 is a panic outside a check's catch, and prints only the fixed line.

**Refusals** come before anything is opened, with exit 2 and one fixed line `e8probe: refused-<reason>`:
- `openalpaca-path`: a component named `.openalpaca` in any letter case, as given or resolved.
- `store-has-discovery-json`, `store-has-daemon-lock`: a path inside a store whose `state/` holds either file (E8 step 2).
- `path-not-canonical`: a path that is not already its own canonical form, that is, a link anywhere on the way (its last component too, even a dangling one), a `..`, or a letter case other than the disk's. So no input is reached through a link. Under `all`, the database it derives, `<store>/state/openalpaca.db`, is checked the same way.
- `state-is-a-link`: a `--store` whose `state` is a link; `store-has-no-state`: under `all`, a `--store` with no `state/` directory.
- `cache-rw-under-state`; `cache-rw-is-cache-ro` (the same file, by device and inode); `cache-rw-is-an-input` (the same file as any other input, the store's own `state/openalpaca.db` and `state/cache/memory-index-v1.sqlite` included); `cache-rw-has-another-link` (a hard link count above 1).
- `output-under-state`, `output-inside-store`, `output-is-an-input`, `out-exists`, `record-exists` (an explicit `search record`).
- `path-unresolvable`, `selftest-knob`, `sha2-known-answer`.

## One command (E8 steps 2–8)

Step 1 is the owner's, typed. `rehearsal.sh --help` prints it:
1. Quit the app.
2. `openalpaca daemon stop`, and check with `pgrep -x openalpacad`.
3. ``T=$(realpath "$(mktemp -d)"); chmod 700 "$T"``.
4. `cp -Rp ~/.openalpaca "$T/realcopy"`.
5. `[ -L "$T/realcopy" ] && echo 'THE COPY IS A LINK - stop here'` and `[ -L "$T/realcopy/state" ] && echo 'ITS state IS A LINK - stop here'`. `cp -Rp` copies a link as a link, so if the store, or its `state/`, is kept behind one (moved to another volume), the copy points at the real files. Then the next line would delete the real discovery file and lock, and every later step would read and open the real store.
6. `rm -f "$T/realcopy/state/discovery.json" "$T/realcopy/state/openalpacad.lock"`.

Then, from the checkout's root:

```sh
/bin/bash tasks/deps-upgrade/tools/e8probe/rehearsal.sh --copy "$T/realcopy" --bins <the folder the lead handed over>
```

The script's header lists every step. In short:
- **Refusals.** The script refuses, first, a `.openalpaca` component in any letter case (as given, then resolved with `/bin/pwd -P`, which gives the disk's letter case where bash's builtin keeps the case typed). It then refuses a copy that is itself a link, and one that holds a link in its top two levels or two levels of its `state/` (`find -P`, which never steps through one), since the doctors' read-write open, the probe and the cache copies would follow it out of the copy. The refusal says how to list them; the way through is to remove each link from the copy (`rm` the link itself, with no trailing slash, never what it names), or to put a copy of what it names in its place (a linked `state/` needs that), and run again. Then it refuses a copy holding either of the two files. It checks `SHA256SUMS` next, and opens nothing of the copy when a sum fails.
- **The children.** Every tool runs under `env -i`: HOME and TMPDIR are in the work dir, `OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR` are the copy's, `NO_COLOR=1`, `LC_ALL=C`, each from a scratch working directory under a time limit.
- **The work dir.** It is a fresh 0700 directory beside the copy, so step 9's `rm -rf "$T"` takes it too.
- **The steps, in order:** step 2b's cache copies; step 3 (the probe's `db` first, then the runbook's sqlite3 block, *Proving read-only*); step 4's inventory; steps 5–6, the two doctors, stdout and stderr apart; steps 7–8, `e8probe-old`, `-new`, `-old`.
- **The comparisons.** The first-line check gives `local` or `VOID`. The doctors' partition keys and health words are compared, and compared with the inventory. The probe runs are compared line by line, the SQLite version lines aside. `copy-memory-unchanged` covers every entry under `memory/`, `projects/` and `state/memory-control/` by stat, before step 3 and after step 8.
- **The verdict:**
  - `void` when a doctor's report came over HTTP;
  - `fail reasons=…` when the doctors' partitions, the probe runs, an integrity or FTS check, `step3-counts`, a key, a note revision or the search disagree, when the copy's memory moved, or when no snapshot was compared (`snapshots-absent`: no control dir; `snapshots-vacuous`: `count=0`; a store that has seen a daemon has at least the home snapshot);
  - `pass` otherwise.

The verdict does not read the schema lines, which the plan names under E8's *Does* and not under its *Gate*. The lead confirms them in the block: each `p<N>-old` and `p<N>-new` run's `db<TAB>schema-version 4` and `index<TAB>schema 6`. An index at 7 means a bumped build has already started on the real store (S12, above), so neither E8's premise nor §5 item 4's backup holds.

`real-daemon-running` is informational and is not a verdict reason. A running real daemon cannot void the doctors, because their branch reads the copy's discovery file, which step 1 removed and the refusal enforces. It decides only whether the copy can be §5 item 4's backup, which requires that no daemon started on the real store since step 1.

The home partitions read `quarantined` in both doctors. A home snapshot binds its absolute root, and the copy is elsewhere (R09), so equal reports are relocation, not healthy reads. Project partitions bind by id and read `ready` in place. The probe's snapshot check never consults `root`, so it reads both kinds.

Stdout holds the block and nothing else; paste what lies between the frames. One stderr line names the work dir. That dir holds the doctors' full reports, step 3's per-table output, the probe's `--out` files and the logs, and is not for pasting. Exit 0 means every step ran. Exit 1 means a step could not run (`step-failed <step>`). Exit 2 is a usage error or a refusal. Exit 130, 143 or 129 means Ctrl-C, TERM or a hangup; the block is still printed.

The block's `script commit <sha> tools-dir-modified yes|no` line names the commit the script ran from. `no` compares this folder's tracked files with that commit (`git diff --quiet HEAD`), so it does not see an untracked file: one added to the folder and never committed still reads `no`. Once the folder is committed, an edit of any of its nine files reads `yes`.

By hand, the runbook's §E8 sequence is the same commands, with these changes, which the runbook's typed form needs too (it lies outside this folder, so this commit does not carry them; the lead applies them):
- step 1, the two `[ -L ]` checks above, before the `rm -f`;
- step 3, the probe's `db` before the sqlite3 block (*Proving read-only*), and every sqlite3 call with `-safe` (no `writefile()`, `edit()` or `ATTACH`, which `mode=ro` does not stop) and the loop's table name with its double quotes doubled, `q=${t//\"/\"\"}`, then `FROM \"$q\"`. A table name comes from the copy's `sqlite_master`.

## Proving read-only

- **The opens.** The probe opens every database `file:<percent-encoded path>?mode=ro` with `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_URI | NO_MUTEX`, then `PRAGMA query_only = 1`. It never uses `immutable=1`, which skips the WAL. It never passes a CREATE flag, so a missing file is an error. Only `fts-rw` opens read-write, without CREATE, on the scratch copy `--cache-rw` names.
- **The sidecars of a WAL database.**
  - The ro open leaves the main file and an existing `-wal` byte-identical. `probe-test.sh` hashes every file of a fresh input before and after `db`, `index`, `search` and `snapshots`, and in the WAL case every file but the `-shm` of a store whose `-wal` holds frames, before and after both variants' reads.
  - A `-shm` may change or appear, since it is a WAL reader's shared-memory index.
  - When the copy has **no** `-wal` and no `-shm`, the bundled SQLite (3.51.1 and 3.53.2 alike) creates a zero-length `-wal` and a `-shm` beside the database, and reads it. That is the normal state of a cleanly stopped daemon's store: its bundled SQLite removes both at the last close.
  - **The system `/usr/bin/sqlite3` (3.54.0) cannot open such a database read-only.** It fails with "unable to open database file (14)". Once the sidecars exist, it reads the database.
  - So E8 step 3's raw read runs the probe's `db` check first (the plan's "or the rebuilt probe in mode=ro") and the sqlite3 block after it. `rehearsal.sh` does this. Typed by hand, the runbook's step 3 needs the same order.
- **Un-checkpointed WAL rows are counted** (R08). `probe-test.sh` commits a row with `wal_autocheckpoint=0` and exits without closing. The probe's `rows-total` is one higher, while `sqlite3 'file:…?immutable=1'` counts one fewer.
- **The recorded root is never read.** `probe-test.sh` rewrites every snapshot's `root` to a decoy folder holding notes with other revisions, and runs three times:
  - with the folder readable, where a probe that followed `root` would read the decoys;
  - with the folder `chmod 000`;
  - under `sandbox-exec` with `(deny file-read* (subpath <decoy>))`, while `log stream --predicate 'sender == "Sandbox"'` runs. A calibration `cat` of the decoy must show in the stream, and no line may name either probe's process. A report names a process by its executable's file name (cut to 16 bytes), `e8probe` when built in place and `e8probe-new` from a bins folder, so the test takes both names from the binaries it runs. This is the only check that sees a metadata-only read (a `stat` of the root changes no count): a mutant that only stats the recorded root, named `e8probe-new`, fails it and nothing else.

  The counts equal the clean run's every time. `log show` after the fact does not hold the kernel's sandbox reports on this machine; the stream does. Only the stream lines naming the decoy folder are kept.
- **No link is followed.** Every path given passes `path-not-canonical` (*Refusals*), a store whose `state` is a link is refused, and the probe's own walks take each entry by `symlink_metadata`. `rehearsal.sh` refuses a copy holding a link where a step would follow it (*One command*).

## Proving the panic path

The panic hook prints one fixed line on stderr, `e8probe: a panic; its message is withheld because it could quote the input`, and never the message. Each check runs under `catch_unwind` and prints `<check>\tpanic`.

`E8_SELFTEST_PANIC=<check>` (`db`, `index`, `fts-rw`, `search`, `snapshots`, `inventory`) makes that check panic on purpose. Its payload is the input's path and first bytes, which is what a library quoting its input would do. Any other value is refused before anything is opened. Never set it on a real run.

`probe-test.sh` runs each knob on both variants with a marker in the path. The test requires `<check>\tpanic`, exit 1, stderr holding only the fixed line, and the marker on neither stream.

## How it is tested

**Corpus B.** `probe-test.sh` builds it fresh for every run:
- the database from the tracked migrations `001`–`004` through `/usr/bin/sqlite3`, then `PRAGMA journal_mode=WAL`, with these rows:
  - the owner id (`identity.local_user_id`) and its `global_user` row;
  - a row holding the marker;
  - three `file_assets` rows: an in-project pointer whose `project_root` names `/nonexistent/MARKER-e8-7f3a`, a retryable `error` and an `uploaded`;
- the index from `memory_index/mod.rs`'s `SCHEMA_SQL`, taken between its anchors with each matched exactly once, plus `create_schema`'s rows: `schema_version` 6 (the pre-wave build's, which the owner's copy holds; the tip's `create_schema` writes 7), `state` ready, and the two `rank` settings read from `RANK_WEIGHTS` and `HEAD_RANK_WEIGHTS`. It holds five notes with chunks and rows in all three FTS tables;
- a home partition with three notes and a tombstone, and a project partition with two notes. The project's folder is named by `folder_name` of a real fixture directory, so the doctors' lookup finds it. Snapshots are written field for field, with revisions computed over each file's bytes (cross-checked with `shasum -a 256`);
- an earlier build's project snapshot (R22): no `project_id`, two recorded notes, no folder. The doctors list it apart. The probe reads `key=absent`, `folder=orphan` and its notes `unlocated`, not `missing`, and the inventory holds its key alone (`snapshot-only=1`);
- the marker `MARKER-e8-7f3a` in note titles and bodies, the owner id, the project folder's name and the copy's own path.

The system sqlite3 keeps a checkpointed zero-length `-wal` and a `-shm` after its last close. The corpus removes both, so it is a cleanly stopped daemon's store. It is then `cp -Rp`'d, so home snapshots bind the original's path, as the owner's copy's do.

**`probe-test.sh`** (`--build-root <dir>` builds both variants first, as above; or `--old` and `--new`, the form a bins folder gives, with the binaries named `e8probe-old` and `e8probe-new`). Every case runs on both variants:

| Case | Expected |
|---|---|
| Old equals new (`all` as old, new, old) | Every word and count line equal, `sqlite 3.51.1` vs `3.53.2`, integrity ok, `fk 0`, schema 4 / 6, `addr-index expected`, `step3-counts equal`, `key=equal` for the two live snapshots, the earlier build's `key=absent folder=orphan unlocated=2 missing=0`, matched 5 and mismatched 0, search `different 0` with results; the inventory equal |
| One flipped byte | `mismatched=1` in that partition |
| A missing file | `missing=1` |
| The recorded root is never read | Counts equal the clean run's (readable decoy, `chmod 000`, under `sandbox-exec`); the calibration read reported, no read reported under either probe's own process name |
| Markers never printed | `MARKER-e8` on neither stream; every stdout line inside `rehearsal.sh`'s alphabet |
| Un-checkpointed WAL rows | `rows-total` +1; `immutable=1` one fewer; every file but the `-shm`, the database and its `-wal` among them, byte-identical across both variants' reads |
| FTS corruption (`fts_word_data`'s last block zeroed) | `fts-rw`: `fts_word corrupt`; `index`: `integrity not-ok` |
| The ro open changes nothing | Main files and an existing `-wal` unchanged; a `-shm` and a new zero-length `-wal` only |
| No sidecars | The system sqlite3 cannot open it read-only; the probe can, and then step 3's block (as `rehearsal.sh` runs it: `env -i`, an empty `HOME`, `-batch`, `-safe`) runs with every call exiting 0 |
| Refusals | A `.openalpaca` component (as given, as `.OpenAlpaca`, through a link), `state/discovery.json`, `state/openalpacad.lock`, a store whose `state/` is a link (`all` and `snapshots`), a database given through a linked `state/`, a store whose `openalpaca.db` is a link, a cache given through a link, `--cache-rw` under `state/`, the same file or a hard link of `--cache-ro`, a hard link of the store's own index or database, another hard link with no `--store`, an existing `--out`, an `--out` inside `--store`, an existing record: exit 2, one fixed line, no `-shm` or `-wal` made, in a link's target either |
| Projects as `list` reads them | Two folders sharing an id and a hidden one: `shared-id=2 hidden=1`, the project partition `folder=orphan`, its notes `unlocated`; a `project.json` without `created_at`, or with a relative `root`: `unreadable=1` |
| `.history` | A link or a file at the home owner folder's `.history`: `notes-dir=link-skipped`, its 3 notes `unlocated`; the project partition untouched |
| The panic path | As above |
| The known answer | `cargo test --locked` per variant: sha256 of `abc`, of the empty string and of FIPS 180-2's two-block message in the one spelling, and the key derivations |

**`rehearsal-test.sh --bins <dir>`** runs `rehearsal.sh` on its own `cp -Rp` of corpus B:
- with the real binaries: the verdict is pass; both doctors read local and equal (home quarantined, the project ready, the earlier build's snapshot listed apart); `doctor-*-vs-inventory missing=1 extra=0`, the inventory's one key that neither doctor reports, that snapshot's; and the block holds no marker, no `.openalpaca` and no dropped line;
- without an index, and with a flipped note byte (`verdict fail` naming note-mismatch);
- with a byte above 0x7F in the copy's path (step 3's URI encoding, `verdict pass`), and without `state/memory-control` (`verdict fail` naming snapshots-absent and snapshots-vacuous);
- through every refusal (`.openalpaca` as given, as `.OpenAlpaca` and `.OPENALPACA/`, through a link and to a folder named `.OpenAlpaca` on disk; a copy that is a link, with and without a trailing slash; a linked `state/`, whose target stays byte-identical; a link at `config/llm.toml` and at a cache sidecar; the two files, as files and as a dangling link), `--help` (its link checks before its `rm -f`), a tampered binary, and a `SHA256SUMS` naming a missing file;
- with E7's folder shape;
- with fake tip doctors: one without the first line (void), one that differs (fail), one that hangs under `--tool-time-limit 2` (timeout);
- with TERM sent to the process group.

## Formats matched (at `0286a80f`, re-read at `19b716ac`)

Between the two, of the files cited below only `memory_index/mod.rs` changed (the sweep's S12: five comment lines and `SCHEMA_VERSION` 6 → 7, above `SCHEMA_SQL`), so its lines are `19b716ac`'s, five below `0286a80f`'s; every other line cited is the same at both.

| What the probe reads | Source |
|---|---|
| `sha256_hex` = lower hex over the digest's bytes | `crates/openalpaca_storage/src/hash.rs:16-25` |
| The owner key: `sha256_hex(owner_id)[..32]`; the project key: `sha256_hex(project_id)[..16]`; `home-<ok>`, `proj-<ok>-<pk>`; the key's shape | `memory_files/mod.rs:108-113` (`OWNER_KEY_HEX`, `DIR_KEY_HEX`), `:140-165` (`owner_key`, `project_key`, `home_partition_key`, `project_partition_key`), `:191-201` (`is_partition_key`) |
| A note's revision is `sha256_hex` of its file's bytes | `memory_files/note.rs:276` (`revision_of`), `partition.rs:497-517` (`classify`) |
| Note files: `<canonical uuid>--<slug>.md`, `*.md` not hidden, regular files only, never through a link | `note.rs:47-56` (`NoteId::parse`), `:350-363` (`file_name_for`, `id_from_file_name`); `dirstat.rs:29` (`is_note_name`), `:50` (`list_by_lstat`); `fsx.rs:86` (`read_regular_nofollow`) |
| The snapshot: `schema`, `partition_key`, `scope` (`home`/`project`), `owner_key`, `owner_id`, `root`, `project_id` (project only), `notes{id: {revision, origin, written_at}}`, `pending{id, from_rev, to_rev, origin}`, `tombstones[{id, …}]`; schema 1 | `memory_files/snapshot.rs:117-122` (`NoteRecord`), `:135-140` (`Pending`), `:147-154` (`Tombstone`), `:159-183` (`ControlSnapshot`), `:186` (`SNAPSHOT_SCHEMA`) |
| Home binds `root`; a project binds `project_id` | `snapshot.rs:274-318` (`check_binding`) |
| `state/memory-control/*.json`, hidden names left out | `partition.rs:581` (`snapshot_path_for`), `:2696` (`list_snapshots`) |
| The notes folder: `<store>/memory/owners/<ok>/notes/`, a project's under `projects/<name>/`; a link on the way refuses | `partition.rs:721` (`open_inner`), `:2673` (`list_owner_keys`) |
| A file named for a tombstoned id is left out of every read | `partition.rs:1202` (`scan_keyed`) |
| `projects/<name>/project.json`: `version` 1, `id` canonical, `root` absolute, `created_at` present, `previous_root`/`previous_name` optional strings; never a link, one hard link | `store/projects.rs:37-41`, `:240` (`ProjectFile`), `:511` (`read_project_dir`), `:552` (the absolute root); `memory_files/fsx.rs:86`, `:98` (`read_regular_nofollow`, the link count), `:218` (`read_if_exists`); the folder name, `store/projects.rs:130` (`folder_name`) |
| `list`: dot-prefixed names passed over, links and files too; two folders carrying one id both left out | `store/projects.rs:698` (`list`), `:723` (the dot names), `:745` (the shared id) |
| A partition's directories, each that exists a real directory: `memory`, `owners`, `<ok>`, `notes`, `.history` (and, for a project, `projects/` and its folder) | `memory_files/partition.rs:599` (`scope_root`), `:721-748` (`open_inner`); `fsx.rs:67` (`real_directory_state`); `memory_files/mod.rs:102` (`HISTORY_DIR`) |
| The index: tables, the three FTS5 tables (`content=''`, `contentless_delete=1`), `meta(schema_version, state)`, version 6 at the branch point and `0286a80f`, 7 at `19b716ac`; `chunk.id` = FTS rowid → `chunk.note_rowid` → `note.id` | `memory_index/mod.rs:95` (`SCHEMA_VERSION`), `:415-511` (`SCHEMA_SQL`), `:515-524`, `:537` (`EXPECTED_TABLES`), `:1559-1564` (the FTS inserts), `:1699` (`create_schema`) |
| The database: schema versions 1–4; 004's address index text | `migrations/mod.rs:23` (`MIGRATIONS`), `migrations/004_project_files.sql:12` |
| Retryable extraction rows: status `error`, retried while `parse_retry_count(extract_error) <= retry_count` | `apps/openalpacad/src/background.rs:102`, `:169` |
| The doctor: `No daemon is running; scanning locally.` on stdout when local; it opens the migrating database and reads `identity.local_user_id`; `print_doctor`'s partition line `<mark> <scope> <key>[  (orphan …)]  <health>` (scope `global` for home) | `apps/openalpaca/src/commands/memory.rs:689-701`, `:1721` (`open_owner_database`), `:2213` (`print_doctor`); `apps/openalpacad/src/bootstrap/migration.rs:38-42` |
| The doctor skips a snapshot whose id no folder records, and continues after a listing error | `memory_files/doctor.rs:156`, `:168` |

## What it does not do

- It never prints a path, a file name, a note id, a title, an owner id, a value or an error message on stdout or stderr.
- It never writes beside its inputs. `--out` and `--record` are refused inside `--store` or a `state/` folder.
- It never opens a snapshot's `root`, a project's recorded root or a `file_assets` path, and never walks below its fixed one-level listings: `state/memory-control/`, `projects/`, `memory/owners/`, `projects/*/memory/owners/`, each `notes/`.
- It never follows a link. A path given through one is refused (`path-not-canonical`), and so is a store whose `state` is one; each entry of its listings is taken by `symlink_metadata`, and a linked one is counted `link-skipped`.
- It never runs `MemoryIndex::open` or any daemon code, never repairs, and never writes a snapshot or a note.
- `rehearsal.sh` never names the real store, never stops or starts a daemon, and never runs a doctor with `--repair`.
- The doctors open the copy's database read-write (the migrating open), which is why step 3's capture is the baseline. The doctor's project lookup also stats each project's recorded root, the real project folder; that is the CLI's own read, not the probe's.
- The tools are macOS first (DU-33): `stat -f` and `sandbox-exec` in the scripts, and O_NOFOLLOW set without a libc dependency so the two manifests stay two lines apart.

## Revision log

| Date | Commit | Change |
|---|---|---|
| 2026-10-29 | (to fill) | **rev 1.** The probe (`src/main.rs`, one source; `old/` rusqlite 0.38.0 + sha2 0.10.9, `new/` rusqlite 0.40.2 + sha2 0.11.1, committed locks), `probe-test.sh`, `rehearsal.sh` and `rehearsal-test.sh`, after runbook §4 and §E8 and programme-plan §4.5 E8 and E8-PROBE, with the lead's readings above (the fourth added on landing). Deviations from the runbook's sketch: (1) step 3 runs the probe's `db` before the sqlite3 block, because the system sqlite3 cannot open a sidecar-less WAL database read-only; (2) the ro open may create a zero-length `-wal` beside a `-shm`; (3) `new/`'s sha2 subtree carries no cmov or ctutils; (4) the sandbox check uses `log stream` with a calibration read, and names the probes by their own file names; (5) refusals and words beyond the sketch's (*What it prints*), `unlocated` among them, so that an orphan snapshot's notes do not read as `missing` and fail the verdict; (6) the `verdict` line; (7) after a review on synthetic stores, before the commit: no link is followed (a path given through one, a linked `state/`, a copy that is one or holds one where a step would follow it, all refused; step 1 checks the copy and its `state/` before its `rm -f`); `.openalpaca` in any letter case; `--cache-rw` never another input and never a hard link; step 3's sqlite3 with `-safe` and its table names quoted; projects read as `list` reads them; `.history` checked as `open_inner` checks it; two verdict reasons for a run that compared no snapshot. The runbook's typed §E8 needs the step 1 and step 3 changes too (*One command*). Landed on `19b716ac`, re-read against it: neither pin moved (rusqlite 0.40.2, sha2 0.11.1); the four shared crates the sweep moved (cc, find-msvc-tools, libc, smallvec) stay at `0286a80f`'s in both locks until a re-pin, which takes `TIP`'s; the index's schema 6 is the pre-wave build's and the owner's, the tip's 7 (S12); the `memory_index/mod.rs` citations five lines down, the others unchanged (`folder_name` cited in `store/projects.rs`, its own file). Added on landing: the re-pin rule's last step (the version checks that follow `new/`) and two notes on the block (*One command*: what `tools-dir-modified` sees, and the schema lines the lead confirms, which the verdict does not read). Verified at `0286a80f` and again at `19b716ac`, each in a scratch worktree, synthetic stores only, with the same counts, as the commit body records. |
