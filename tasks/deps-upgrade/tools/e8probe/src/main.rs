//! e8probe — E8 step 7's read-only checks of a COPY of a store (programme-plan
//! §4.5 *E8* and *E8-PROBE*). One source, two builds that differ only in
//! rusqlite and sha2: `old/` (rusqlite 0.38.0 with the bundled SQLite 3.51.1,
//! sha2 0.10.9 — the branch point's) and `new/` (rusqlite 0.40.2 with SQLite
//! 3.53.2, sha2 0.11.1 — the wave's). README.md beside this directory is the
//! manual; every format below was matched against the source it cites there.
//!
//! It prints only fixed words, counts and partition keys (F-04) on stdout, one
//! `<check>\t...` line each; everything finer (table names, note ids, file
//! names, project roots) goes to the `--out` file the caller names under its
//! scratch root, which the caller never pastes. On stderr it prints only a
//! usage line, one fixed refusal line, or the fixed panic line.
//!
//! Never: `immutable=1` (it skips the copied WAL and undercounts, R08), a
//! `MemoryIndex::open` (which would recreate a cache), a write to a snapshot or
//! a note, a `doctor --repair`, a read of a snapshot's recorded `root` or of a
//! project's recorded root, a stat of a `file_assets` path, a walk below the
//! fixed one-level listings, a followed link (a path given through one is
//! refused, and a store whose `state/` is one too). The one write to a database is
//! FTS5's `integrity-check` `INSERT`, on the read-write SCRATCH copy of the
//! cache that `--cache-rw` names.
//!
//!   e8probe version
//!   e8probe db        --db <copy>/state/openalpaca.db --out <f> [--step3 <f>]
//!   e8probe index     --cache <ro copy> --out <f>
//!   e8probe fts-rw    --cache-rw <rw scratch copy> --cache-ro <ro copy>
//!   e8probe search    record  --cache <ro copy> --record <f>
//!   e8probe search    compare --cache <ro copy> --record <f>
//!   e8probe snapshots --store <copy root> --out <f>
//!   e8probe inventory --store <copy root> --out <f>
//!   e8probe all       --store <copy root> --cache-ro <f> --cache-rw <f> --record <f> --out <f> [--step3 <f>]
//!
//! `all` runs db (on `<store>/state/openalpaca.db`), index, fts-rw, search and
//! snapshots; its search records when `--record` does not exist yet and
//! compares when it does, so E8's `for v in old new old` loop records once and
//! compares twice. A `--cache-ro` that does not exist reads `absent` for the
//! three cache checks.
//!
//! Exit status: 0 when every check ran (a `not-ok`, `different` or `corrupt`
//! is a result); 1 when a check printed `error` or `panic`; 2 for a usage error
//! or a refusal, before anything is opened; 101 for a panic outside a check's
//! catch (only the fixed line is printed).

use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// The schema versions `migrations::MIGRATIONS` registers at the tip and at
/// the branch point (`crates/openalpaca_storage/src/migrations/mod.rs`).
const DB_SCHEMA_SET: &str = "1,2,3,4";
/// The pre-wave build's `memory_index::SCHEMA_VERSION` (6 at the branch
/// point), as its `create_schema` writes it into
/// `meta(key = 'schema_version')` (`memory_index/mod.rs`): the version the
/// owner's index holds when E8 copies it. The tip's is 7 (the sweep's S12,
/// no table changed), and E8 runs before a bumped build first starts on the
/// real store, so a copy reading 7 was written by a bumped build.
const INDEX_SCHEMA: &str = "6";
/// The three FTS5 tables of `memory_index/mod.rs`'s `SCHEMA_SQL`.
const FTS: [&str; 3] = ["fts_word", "fts_tri_head", "fts_tri"];
/// The index's other tables whose rows are counted.
const INDEX_TABLES: [&str; 5] = ["note", "chunk", "note_alias", "note_source", "partition"];
/// Migration 004's address index, as SQLite stores its CREATE text
/// (`migrations/004_project_files.sql`, the statement without its `;`).
const ADDR_INDEX: &str = "idx_file_assets_addr";
const ADDR_INDEX_SQL: &str = "CREATE UNIQUE INDEX idx_file_assets_addr ON file_assets(COALESCE(project_root, ''), in_project, rel_path) WHERE rel_path IS NOT NULL";
/// `memory_files::OWNER_KEY_HEX` and `DIR_KEY_HEX` (`memory_files/mod.rs`).
const OWNER_KEY_HEX: usize = 32;
const DIR_KEY_HEX: usize = 16;
/// A note file is read whole up to this; a larger one counts `oversize`, no
/// hash (the daemon's own limit is 32 KiB, `MAX_NOTE_BYTES`).
const NOTE_READ_CAP: u64 = 64 * 1024 * 1024;
/// A snapshot or a `project.json` is read whole up to this.
const SMALL_READ_CAP: u64 = 16 * 1024 * 1024;
/// How many note rows, by rowid, give the search's derived queries.
const SEARCH_NOTES: i64 = 200;
/// The fixed queries beside the derived ones: words on `fts_word`, trigrams on
/// `fts_tri` (the body table, which the derived queries do not reach).
const FIXED_WORDS: [&str; 6] = ["the", "user", "project", "memory", "prefers", "uses"];
const FIXED_TRIGRAMS: [&str; 3] = ["the", "ing", "ion"];
const RECORD_HEAD: &str = "# e8probe search record v1";
const PANIC_LINE: &str =
    "e8probe: a panic; its message is withheld because it could quote the input";
/// FIPS 180-2's "abc".
const KAT_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const CHECKS: [&str; 6] = ["db", "index", "fts-rw", "search", "snapshots", "inventory"];

static FAILED: AtomicBool = AtomicBool::new(false);

// ---- sha2: the one spelling OpenAlpaca writes ---------------------------------

/// `openalpaca_storage::hash::sha256_hex` (`crates/openalpaca_storage/src/hash.rs`):
/// lower hex, two digits a byte, written over the digest's bytes — never
/// `{:x}`/`{:?}` of the digest type, whose spelling differs across sha2's
/// GenericArray (0.10) and hybrid-array (0.11) outputs.
fn sha256_hex(b: &[u8]) -> String {
    lower_hex(&Sha256::digest(b))
}

fn lower_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---- output -------------------------------------------------------------------

/// One stdout line: fixed words, counts, partition keys only.
fn say(check: &str, rest: &str) {
    println!("{check}\t{rest}");
}

/// The `--out` file: the detail the caller keeps and never pastes. Created
/// new, 0600; a write failure is that check's `error`.
struct Detail {
    file: Option<fs::File>,
}

impl Detail {
    fn none() -> Self {
        Self { file: None }
    }

    fn create(p: &Path) -> Self {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc_o_nofollow())
            .open(p)
        {
            Ok(file) => Self { file: Some(file) },
            Err(_) => refuse("out-not-creatable"),
        }
    }

    fn line(&mut self, s: &str) -> Result<(), String> {
        if let Some(f) = self.file.as_mut() {
            writeln!(f, "{s}").map_err(|_| "out-write".to_string())?;
        }
        Ok(())
    }
}

/// O_NOFOLLOW, without a libc dependency (the two variants' manifests differ
/// in rusqlite and sha2 alone). Every open also checks `symlink_metadata`
/// first, so a platform without a value here still never follows a link it
/// was given.
const fn libc_o_nofollow() -> i32 {
    if cfg!(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd"
    )) {
        0x0100
    } else if cfg!(all(
        target_os = "linux",
        any(target_arch = "aarch64", target_arch = "arm")
    )) {
        0o100000
    } else if cfg!(target_os = "linux") {
        0o400000
    } else {
        0
    }
}

// ---- refusals and usage ---------------------------------------------------------

/// Exit 2 with one fixed line; `reason` is one of this file's literals.
fn refuse(reason: &str) -> ! {
    let _ = writeln!(std::io::stderr(), "e8probe: refused-{reason}");
    std::process::exit(2);
}

fn usage() -> ! {
    let _ = writeln!(
        std::io::stderr(),
        "usage: e8probe version | db --db <f> --out <f> [--step3 <f>] | index --cache <f> --out <f> | \
         fts-rw --cache-rw <f> --cache-ro <f> | search record|compare --cache <f> --record <f> | \
         snapshots --store <d> --out <f> | inventory --store <d> --out <f> | \
         all --store <d> --cache-ro <f> --cache-rw <f> --record <f> --out <f> [--step3 <f>]"
    );
    std::process::exit(2);
}

// ---- arguments -----------------------------------------------------------------

#[derive(Default)]
struct Args {
    cmd: String,
    mode: Option<String>,
    flags: BTreeMap<String, PathBuf>,
}

const PATH_FLAGS: [&str; 8] = [
    "--db",
    "--out",
    "--step3",
    "--cache",
    "--cache-ro",
    "--cache-rw",
    "--record",
    "--store",
];

impl Args {
    fn parse() -> Self {
        let raw: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        let mut it = raw.into_iter();
        let cmd = match it.next().and_then(|c| c.into_string().ok()) {
            Some(c) => c,
            None => usage(),
        };
        let mut a = Args {
            cmd,
            ..Args::default()
        };
        let (required, optional): (&[&str], &[&str]) = match a.cmd.as_str() {
            "version" => (&[], &[]),
            "db" => (&["--db", "--out"], &["--step3"]),
            "index" => (&["--cache", "--out"], &[]),
            "fts-rw" => (&["--cache-rw", "--cache-ro"], &[]),
            "search" => (&["--cache", "--record"], &[]),
            "snapshots" | "inventory" => (&["--store", "--out"], &[]),
            "all" => (
                &["--store", "--cache-ro", "--cache-rw", "--record", "--out"],
                &["--step3"],
            ),
            _ => usage(),
        };
        if a.cmd == "search" {
            match it.next().and_then(|m| m.into_string().ok()).as_deref() {
                Some("record") => a.mode = Some("record".into()),
                Some("compare") => a.mode = Some("compare".into()),
                _ => usage(),
            }
        }
        while let Some(flag) = it.next() {
            let Some(flag) = flag.to_str().map(str::to_string) else {
                usage()
            };
            if !(required.contains(&flag.as_str()) || optional.contains(&flag.as_str()))
                || !PATH_FLAGS.contains(&flag.as_str())
            {
                usage();
            }
            let Some(value) = it.next() else { usage() };
            if value.is_empty() || a.flags.contains_key(&flag) {
                usage();
            }
            a.flags.insert(flag, PathBuf::from(value));
        }
        for r in required {
            if !a.flags.contains_key(*r) {
                usage();
            }
        }
        a
    }

    fn get(&self, f: &str) -> Option<&Path> {
        self.flags.get(f).map(PathBuf::as_path)
    }
}

// ---- the guard: every path, before anything is opened ---------------------------

/// A component named `.openalpaca` in any letter case: the default APFS
/// volume is case-insensitive, so `.OpenAlpaca` names the same folder.
fn has_openalpaca_component(p: &Path) -> bool {
    p.components().any(
        |c| matches!(c, Component::Normal(n) if n.as_bytes().eq_ignore_ascii_case(b".openalpaca")),
    )
}

/// The path made absolute and canonical as far as it exists: the longest
/// existing ancestor canonicalized, the rest appended. `None` when no
/// ancestor resolves or a remaining component is `..`.
fn canonical_lenient(p: &Path) -> Option<PathBuf> {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(p)
    };
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = abs.as_path();
    loop {
        if let Ok(c) = fs::canonicalize(cur) {
            let mut out = c;
            for r in rest.iter().rev() {
                if r.as_bytes() == b".." {
                    return None;
                }
                if r.as_bytes() != b"." {
                    out.push(r);
                }
            }
            return Some(out);
        }
        rest.push(cur.file_name()?.to_os_string());
        cur = cur.parent()?;
    }
}

fn exists_nofollow(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// A live store's marks, E8 step 2's assertion: any ancestor directory named
/// `state` holding `discovery.json` or `openalpacad.lock`, and for a store
/// root its own `state/` when that is a real directory (a linked one is
/// never stepped through; [`guard`] refuses it).
fn live_store_mark(p: &Path) -> Option<&'static str> {
    let own = p.join("state");
    let mut dirs: Vec<PathBuf> = Vec::new();
    if fs::symlink_metadata(&own).is_ok_and(|m| m.is_dir()) {
        dirs.push(own);
    }
    for a in p.ancestors() {
        if a.file_name()
            .map(|n| n.as_bytes() == b"state")
            .unwrap_or(false)
        {
            dirs.push(a.to_path_buf());
        }
    }
    for d in dirs {
        if exists_nofollow(&d.join("discovery.json")) {
            return Some("store-has-discovery-json");
        }
        if exists_nofollow(&d.join("openalpacad.lock")) {
            return Some("store-has-daemon-lock");
        }
    }
    None
}

fn dev_ino(p: &Path) -> Option<(u64, u64)> {
    fs::symlink_metadata(p).ok().map(|m| (m.dev(), m.ino()))
}

/// One path, before anything is opened, returned canonical. Refuses a
/// component named `.openalpaca` (as given or canonical), a path inside a
/// live store, and a path that is not already its own canonical form: a link
/// anywhere on the way (the last component too, even a dangling one), a
/// `..`, or a letter case other than the disk's. No input is ever reached
/// through a link.
fn checked_path(p: &Path) -> PathBuf {
    if has_openalpaca_component(p) {
        refuse("openalpaca-path");
    }
    let Some(c) = canonical_lenient(p) else {
        refuse("path-unresolvable")
    };
    if has_openalpaca_component(&c) {
        refuse("openalpaca-path");
    }
    if let Some(mark) = live_store_mark(&c) {
        refuse(mark);
    }
    let given = if p.is_absolute() {
        p.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(d) => d.join(p),
            Err(_) => refuse("path-unresolvable"),
        }
    };
    let leaf_is_link = fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink());
    if given != c || leaf_is_link {
        refuse("path-not-canonical");
    }
    c
}

/// Refuses (exit 2, one fixed line) before anything is opened:
/// - each flag's path, by [`checked_path`] — the probe runs on a copy, never
///   on the live store, and never through a link;
/// - a `--store` whose `state` is a link, and under `all` one with no real
///   `state/` directory; `all`'s `<store>/state/openalpaca.db`, the one input
///   it derives rather than takes, passes [`checked_path`] as a flag's does;
/// - a `--cache-rw` inside any `state/` directory, the same file as
///   `--cache-ro` or as any other input (the store's own database and index
///   included), or with another hard link;
/// - an `--out` or `--record` that would be written inside `--store` or a
///   `state/` directory, or an `--out` that exists;
/// - a `search record` whose `--record` exists.
fn guard(a: &mut Args) {
    let mut canon: BTreeMap<String, PathBuf> = BTreeMap::new();
    for (flag, p) in &a.flags {
        canon.insert(flag.clone(), checked_path(p));
    }
    // The store's own database and index: `all` opens the first, and the
    // read-write scratch copy may be neither.
    let mut store_files: Vec<PathBuf> = Vec::new();
    if let Some(store) = canon.get("--store").cloned() {
        let state = store.join("state");
        match fs::symlink_metadata(&state) {
            Ok(m) if m.file_type().is_symlink() => refuse("state-is-a-link"),
            Ok(m) if m.is_dir() => {}
            _ if a.cmd == "all" => refuse("store-has-no-state"),
            _ => {}
        }
        if a.cmd == "all" {
            let db = checked_path(&state.join("openalpaca.db"));
            canon.insert("--db".into(), db);
        }
        store_files.push(state.join("openalpaca.db"));
        store_files.push(state.join("cache").join("memory-index-v1.sqlite"));
    }
    let under_state = |p: &Path| {
        p.components()
            .any(|c| matches!(c, Component::Normal(n) if n.as_bytes() == b"state"))
    };
    if let Some(rw) = canon.get("--cache-rw") {
        if under_state(rw) {
            refuse("cache-rw-under-state");
        }
        if let Some(ri) = dev_ino(rw) {
            if canon.get("--cache-ro").and_then(|ro| dev_ino(ro)) == Some(ri) {
                refuse("cache-rw-is-cache-ro");
            }
            if canon
                .iter()
                .filter(|(f, _)| f.as_str() != "--cache-rw")
                .map(|(_, p)| p)
                .chain(store_files.iter())
                .any(|p| dev_ino(p) == Some(ri))
            {
                refuse("cache-rw-is-an-input");
            }
            if fs::symlink_metadata(rw).is_ok_and(|m| m.nlink() > 1) {
                refuse("cache-rw-has-another-link");
            }
        }
    }
    for w in ["--out", "--record"] {
        let Some(p) = canon.get(w) else { continue };
        if under_state(p) {
            refuse("output-under-state");
        }
        if let Some(store) = canon.get("--store")
            && p.starts_with(store)
        {
            refuse("output-inside-store");
        }
        for (flag, other) in &canon {
            if flag != w && dev_ino(other).is_some() && dev_ino(other) == dev_ino(p) {
                refuse("output-is-an-input");
            }
        }
    }
    if let Some(p) = canon.get("--out")
        && exists_nofollow(p)
    {
        refuse("out-exists");
    }
    if a.mode.as_deref() == Some("record")
        && let Some(p) = canon.get("--record")
        && exists_nofollow(p)
    {
        refuse("record-exists");
    }
    a.flags = canon;
}

/// `E8_SELFTEST_PANIC=<check>`: that check panics on purpose, with its
/// input's path as the payload — what a library quoting its input would do.
/// Any other value is refused before anything is opened.
fn selftest_knob() -> Option<String> {
    let raw = std::env::var_os("E8_SELFTEST_PANIC")?;
    match raw.to_str() {
        Some(s) if CHECKS.contains(&s) => Some(s.to_string()),
        _ => refuse("selftest-knob"),
    }
}

fn maybe_panic(knob: &Option<String>, check: &str, input: &Path) {
    if knob.as_deref() == Some(check) {
        let text = fs::read(input)
            .map(|b| String::from_utf8_lossy(&b[..b.len().min(256)]).into_owned())
            .unwrap_or_default();
        panic!("selftest {check}: {} {text}", input.display());
    }
}

/// Runs one check under `catch_unwind`: `<check>\tpanic` for a panic (the
/// hook printed only its fixed line), `<check>\terror` for an error (its
/// class goes to `--out`, never stdout).
fn run_check(check: &str, detail: &mut Detail, f: impl FnOnce(&mut Detail) -> Result<(), String>) {
    match catch_unwind(AssertUnwindSafe(|| f(detail))) {
        Ok(Ok(())) => {}
        Ok(Err(class)) => {
            FAILED.store(true, Ordering::SeqCst);
            say(check, "error");
            let _ = detail.line(&format!("{check}\terror\t{class}"));
        }
        Err(_) => {
            FAILED.store(true, Ordering::SeqCst);
            say(check, "panic");
            let _ = detail.line(&format!("{check}\tpanic"));
        }
    }
}

// ---- SQLite ---------------------------------------------------------------------

/// `file:<percent-encoded absolute path>?mode=ro` — never `immutable=1`.
fn uri_ro(p: &Path) -> String {
    let mut s = String::from("file:");
    for &b in p.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s.push_str("?mode=ro");
    s
}

/// SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_URI | NO_MUTEX on `mode=ro`, then
/// `PRAGMA query_only = 1`. No CREATE flag: a missing file is an error, never
/// a new database.
fn open_ro(p: &Path) -> Result<Connection, String> {
    let c = Connection::open_with_flags(
        uri_ro(p),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| sql_class("open", &e))?;
    c.execute_batch("PRAGMA query_only = 1;")
        .map_err(|e| sql_class("query-only", &e))?;
    Ok(c)
}

/// An error's class for `--out`: the step and SQLite's code, never its message
/// (a message can quote a path or a value).
fn sql_class(step: &str, e: &rusqlite::Error) -> String {
    match e {
        rusqlite::Error::SqliteFailure(f, _) => {
            format!(
                "{step} sqlite-code={:?} extended={}",
                f.code, f.extended_code
            )
        }
        _ => format!("{step} rusqlite-other"),
    }
}

fn is_corrupt(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == ErrorCode::DatabaseCorrupt)
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn count(c: &Connection, table: &str) -> Result<i64, String> {
    c.query_row(
        &format!("SELECT count(*) FROM {}", quote_ident(table)),
        [],
        |r| r.get(0),
    )
    .map_err(|e| sql_class("count", &e))
}

fn table_exists(c: &Connection, name: &str) -> Result<bool, String> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [name],
        |r| r.get(0),
    )
    .map_err(|e| sql_class("master", &e))
}

/// `PRAGMA integrity_check`: `ok` only for exactly one row reading `ok`.
/// Since SQLite 3.44 it runs FTS5's own check read-only too.
fn integrity(c: &Connection, detail: &mut Detail, label: &str) -> Result<&'static str, String> {
    let mut st = c
        .prepare("PRAGMA integrity_check")
        .map_err(|e| sql_class("integrity", &e))?;
    let rows: Vec<String> = st
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| sql_class("integrity", &e))?
        .collect::<Result<_, _>>()
        .map_err(|e| sql_class("integrity", &e))?;
    detail.line(&format!("{label}\tintegrity-rows\t{}", rows.len()))?;
    for r in &rows {
        detail.line(&format!("{label}\tintegrity\t{r}"))?;
    }
    Ok(if rows.len() == 1 && rows[0] == "ok" {
        "ok"
    } else {
        "not-ok"
    })
}

// ---- db -------------------------------------------------------------------------

/// The per-table counts as step 3's sqlite3 loop writes them
/// (`<table>\t<count>` lines; the other lines of step 3 hold no tab).
fn read_step3(p: &Path) -> Result<BTreeMap<String, i64>, String> {
    let bytes = read_capped(p, SMALL_READ_CAP).map_err(|_| "step3-read".to_string())?;
    let mut m = BTreeMap::new();
    for line in String::from_utf8_lossy(&bytes).lines() {
        if let Some((name, n)) = line.split_once('\t')
            && !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
            && let Ok(v) = n.parse::<i64>()
        {
            m.insert(name.to_string(), v);
        }
    }
    Ok(m)
}

fn check_db(
    db: &Path,
    step3: Option<&Path>,
    detail: &mut Detail,
    knob: &Option<String>,
) -> Result<(), String> {
    maybe_panic(knob, "db", db);
    let c = open_ro(db)?;
    say("db", &format!("sqlite {}", rusqlite::version()));
    say("db", &format!("integrity {}", integrity(&c, detail, "db")?));
    let fk: i64 = {
        let mut st = c
            .prepare("PRAGMA foreign_key_check")
            .map_err(|e| sql_class("fk", &e))?;
        let mut rows = st.query([]).map_err(|e| sql_class("fk", &e))?;
        let mut n = 0;
        while rows.next().map_err(|e| sql_class("fk", &e))?.is_some() {
            n += 1;
        }
        n
    };
    say("db", &format!("fk {fk}"));
    let max: Option<i64> = c
        .query_row("SELECT max(version) FROM schema_version", [], |r| r.get(0))
        .map_err(|e| sql_class("schema-version", &e))?;
    say(
        "db",
        &format!(
            "schema-version {}",
            max.map(|v| v.to_string())
                .unwrap_or_else(|| "absent".into())
        ),
    );
    let set: Vec<i64> = {
        let mut st = c
            .prepare("SELECT version FROM schema_version ORDER BY version")
            .map_err(|e| sql_class("schema-set", &e))?;
        st.query_map([], |r| r.get::<_, i64>(0))
            .map_err(|e| sql_class("schema-set", &e))?
            .collect::<Result<_, _>>()
            .map_err(|e| sql_class("schema-set", &e))?
    };
    let joined = set.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    say(
        "db",
        &format!(
            "schema-set {}",
            if joined == DB_SCHEMA_SET {
                DB_SCHEMA_SET
            } else {
                "other"
            }
        ),
    );
    detail.line(&format!("db\tschema-set\t{joined}"))?;

    let master: Vec<(String, String, Option<String>)> = {
        let mut st = c
            .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
            .map_err(|e| sql_class("master", &e))?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| sql_class("master", &e))?
            .collect::<Result<_, _>>()
            .map_err(|e| sql_class("master", &e))?
    };
    let kind = |k: &str| {
        master
            .iter()
            .filter(|(t, n, _)| t == k && !n.starts_with("sqlite_"))
            .count()
    };
    say(
        "db",
        &format!(
            "master tables={} indexes={} triggers={} views={}",
            kind("table"),
            kind("index"),
            kind("trigger"),
            kind("view")
        ),
    );
    for (t, n, sql) in &master {
        detail.line(&format!(
            "db\tmaster\t{t}\t{n}\t{}",
            sha256_hex(sql.as_deref().unwrap_or("").as_bytes())
        ))?;
    }
    let addr = match master
        .iter()
        .find(|(t, n, _)| t == "index" && n == ADDR_INDEX)
    {
        None => "absent",
        Some((_, _, Some(sql))) if sql == ADDR_INDEX_SQL => "expected",
        Some(_) => "different",
    };
    say("db", &format!("addr-index {addr}"));

    // The same table list and the same counts as step 3's sqlite3 loop.
    let tables: Vec<String> = {
        let mut st = c
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name;")
            .map_err(|e| sql_class("tables", &e))?;
        st.query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| sql_class("tables", &e))?
            .collect::<Result<_, _>>()
            .map_err(|e| sql_class("tables", &e))?
    };
    let mut counts = BTreeMap::new();
    let mut total: i64 = 0;
    for t in &tables {
        let n = count(&c, t)?;
        total += n;
        counts.insert(t.clone(), n);
        detail.line(&format!("db\trows\t{t}\t{n}"))?;
    }
    say("db", &format!("tables {}", tables.len()));
    say("db", &format!("rows-total {total}"));
    if counts.contains_key("file_assets") {
        let (rows, roots): (i64, i64) = c
            .query_row(
                "SELECT count(*), count(DISTINCT project_root) FROM file_assets WHERE in_project = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| sql_class("in-project", &e))?;
        say("db", &format!("in-project rows={rows} roots={roots}"));
        let by = |s: &str| -> Result<i64, String> {
            c.query_row(
                "SELECT count(*) FROM file_assets WHERE status = ?1",
                [s],
                |r| r.get(0),
            )
            .map_err(|e| sql_class("extraction", &e))
        };
        say(
            "db",
            &format!(
                "extraction error={} uploaded={} processing={}",
                by("error")?,
                by("uploaded")?,
                by("processing")?
            ),
        );
    } else {
        say("db", "in-project absent");
        say("db", "extraction absent");
    }
    if let Some(s3) = step3 {
        let base = read_step3(s3)?;
        let word = if base == counts { "equal" } else { "different" };
        say("db", &format!("step3-counts {word}"));
        for (t, n) in &counts {
            let b = base
                .get(t)
                .map(i64::to_string)
                .unwrap_or_else(|| "absent".into());
            detail.line(&format!("db\tstep3\t{t}\tprobe={n}\tstep3={b}"))?;
        }
    }
    Ok(())
}

// ---- index ----------------------------------------------------------------------

fn small_word(v: Option<String>, allowed: &[&str]) -> String {
    match v {
        None => "absent".into(),
        Some(s) if allowed.contains(&s.as_str()) => s,
        Some(s) if !s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit()) => s,
        Some(_) => "other".into(),
    }
}

fn check_index(cache: &Path, detail: &mut Detail, knob: &Option<String>) -> Result<(), String> {
    maybe_panic(knob, "index", cache);
    let c = open_ro(cache)?;
    say("index", &format!("sqlite {}", rusqlite::version()));
    say(
        "index",
        &format!("integrity {}", integrity(&c, detail, "index")?),
    );
    let meta = table_exists(&c, "meta")?;
    let get_meta = |k: &str| -> Result<Option<String>, String> {
        if !meta {
            return Ok(None);
        }
        c.query_row("SELECT value FROM meta WHERE key = ?1", [k], |r| r.get(0))
            .optional()
            .map_err(|e| sql_class("meta", &e))
    };
    let schema = small_word(get_meta("schema_version")?, &[]);
    say("index", &format!("schema {schema}"));
    detail.line(&format!(
        "index\tschema-expected\t{}",
        schema == INDEX_SCHEMA
    ))?;
    say(
        "index",
        &format!(
            "state {}",
            small_word(get_meta("state")?, &["ready", "rebuilding"])
        ),
    );
    let mut fts = 0;
    for t in FTS {
        let sql: Option<Option<String>> = c
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [t],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| sql_class("fts", &e))?;
        if let Some(Some(sql)) = sql {
            let l = sql.to_ascii_lowercase();
            if l.starts_with("create virtual table") && l.contains("using fts5") {
                fts += 1;
            }
        }
    }
    say("index", &format!("fts-tables {fts}"));
    let mut parts = Vec::new();
    for t in INDEX_TABLES {
        if table_exists(&c, t)? {
            parts.push(format!("{t}={}", count(&c, t)?));
        } else {
            parts.push(format!("{t}=absent"));
        }
    }
    say("index", &format!("rows {}", parts.join(" ")));
    Ok(())
}

// ---- fts-rw ---------------------------------------------------------------------

/// FTS5's `integrity-check` command on the read-write SCRATCH copy: an
/// `INSERT`, which a `mode=ro` connection refuses ("attempt to write a
/// readonly database"). Opened READ_WRITE without CREATE.
fn check_fts_rw(rw: &Path, detail: &mut Detail, knob: &Option<String>) -> Result<(), String> {
    maybe_panic(knob, "fts-rw", rw);
    let c = Connection::open_with_flags(
        rw,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| sql_class("open-rw", &e))?;
    for t in FTS {
        if !table_exists(&c, t)? {
            say("fts-rw", &format!("{t} absent"));
            continue;
        }
        let q = format!(
            "INSERT INTO {0}({0}) VALUES('integrity-check')",
            quote_ident(t)
        );
        let word = match c.execute(&q, []) {
            Ok(_) => "ok",
            Err(e) if is_corrupt(&e) => {
                detail.line(&format!(
                    "fts-rw\t{t}\t{}",
                    sql_class("integrity-check", &e)
                ))?;
                "corrupt"
            }
            Err(e) => {
                detail.line(&format!(
                    "fts-rw\t{t}\t{}",
                    sql_class("integrity-check", &e)
                ))?;
                FAILED.store(true, Ordering::SeqCst);
                "error"
            }
        };
        say("fts-rw", &format!("{t} {word}"));
    }
    Ok(())
}

// ---- search ---------------------------------------------------------------------

/// The first run of at least two alphanumeric characters.
fn first_word(s: &str) -> Option<String> {
    let mut cur = String::new();
    for ch in s.chars().chain(std::iter::once(' ')) {
        if ch.is_alphanumeric() {
            cur.push(ch);
        } else {
            if cur.chars().count() >= 2 {
                return Some(cur);
            }
            cur.clear();
        }
    }
    None
}

/// The queries: for each of the first 200 note rows by rowid, the first word
/// of `norm_title` as an FTS5 string on `fts_word`, and its first three
/// characters on `fts_tri_head`; then the fixed words on `fts_word` and the
/// fixed trigrams on `fts_tri`.
fn queries(c: &Connection) -> Result<Vec<(&'static str, String)>, String> {
    let mut out = Vec::new();
    if table_exists(c, "note")? {
        let titles: Vec<String> = {
            let mut st = c
                .prepare("SELECT norm_title FROM note ORDER BY rowid LIMIT ?1")
                .map_err(|e| sql_class("queries", &e))?;
            st.query_map([SEARCH_NOTES], |r| r.get::<_, String>(0))
                .map_err(|e| sql_class("queries", &e))?
                .collect::<Result<_, _>>()
                .map_err(|e| sql_class("queries", &e))?
        };
        for t in titles {
            if let Some(w) = first_word(&t) {
                if w.chars().count() >= 3 {
                    let tri: String = w.chars().take(3).collect();
                    out.push(("fts_word", w));
                    out.push(("fts_tri_head", tri));
                } else {
                    out.push(("fts_word", w));
                }
            }
        }
    }
    for w in FIXED_WORDS {
        out.push(("fts_word", w.to_string()));
    }
    for w in FIXED_TRIGRAMS {
        out.push(("fts_tri", w.to_string()));
    }
    Ok(out)
}

/// One query's sorted note ids, or an error class.
fn run_query(c: &Connection, table: &str, term: &str) -> Result<Vec<String>, String> {
    let sql = format!(
        "SELECT DISTINCT n.id FROM {t} f JOIN chunk c ON c.id = f.rowid JOIN note n ON n.rowid = c.note_rowid \
         WHERE {t} MATCH ?1 ORDER BY n.id",
        t = quote_ident(table)
    );
    let expr = format!("\"{}\"", term.replace('"', "\"\""));
    let mut st = c.prepare(&sql).map_err(|e| sql_class("search", &e))?;
    st.query_map([expr], |r| r.get::<_, String>(0))
        .map_err(|e| sql_class("search", &e))?
        .collect::<Result<_, _>>()
        .map_err(|e| sql_class("search", &e))
}

struct QueryResult {
    ok: bool,
    n: usize,
    hash: String,
}

fn search_results(c: &Connection) -> Result<(String, Vec<QueryResult>), String> {
    let qs = queries(c)?;
    let list: Vec<String> = qs.iter().map(|(t, w)| format!("{t}:{w}")).collect();
    let list_hash = sha256_hex(list.join("\n").as_bytes());
    let mut out = Vec::new();
    for (t, w) in &qs {
        match run_query(c, t, w) {
            Ok(ids) => out.push(QueryResult {
                ok: true,
                n: ids.len(),
                hash: sha256_hex(ids.join("\n").as_bytes()),
            }),
            Err(_) => out.push(QueryResult {
                ok: false,
                n: 0,
                hash: "-".into(),
            }),
        }
    }
    Ok((list_hash, out))
}

fn check_search(
    cache: &Path,
    record: &Path,
    compare: bool,
    all: bool,
    knob: &Option<String>,
) -> Result<(), String> {
    maybe_panic(knob, "search", cache);
    let c = open_ro(cache)?;
    if !table_exists(&c, "chunk")? || !table_exists(&c, "note")? {
        say("search", "absent");
        return Ok(());
    }
    let (list_hash, results) = search_results(&c)?;
    let with = results.iter().filter(|r| r.ok && r.n > 0).count();
    let errors = results.iter().filter(|r| !r.ok).count();
    if !compare {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc_o_nofollow())
            .open(record)
            .map_err(|_| "record-create".to_string())?;
        let mut text = format!("{RECORD_HEAD}\nqueries\t{}\t{list_hash}\n", results.len());
        for (i, r) in results.iter().enumerate() {
            text.push_str(&format!(
                "{}\t{}\t{}\t{}\n",
                i + 1,
                if r.ok { "ok" } else { "error" },
                r.n,
                r.hash
            ));
        }
        f.write_all(text.as_bytes())
            .map_err(|_| "record-write".to_string())?;
        say(
            "search",
            &format!(
                "recorded queries {} with-results {with} error {errors}",
                results.len()
            ),
        );
        if errors > 0 {
            FAILED.store(true, Ordering::SeqCst);
        }
        return Ok(());
    }
    let bytes = read_capped(record, SMALL_READ_CAP).map_err(|_| "record-read".to_string())?;
    let text = String::from_utf8(bytes).map_err(|_| "record-not-utf8".to_string())?;
    let mut lines = text.lines();
    if lines.next() != Some(RECORD_HEAD) {
        say("search", "record-refused");
        FAILED.store(true, Ordering::SeqCst);
        return Ok(());
    }
    let head: Vec<&str> = lines.next().unwrap_or("").split('\t').collect();
    let list_word = if head.len() == 3
        && head[0] == "queries"
        && head[1] == results.len().to_string()
        && head[2] == list_hash
    {
        "equal"
    } else {
        "different"
    };
    say("search", &format!("query-list {list_word}"));
    let rec: Vec<Vec<&str>> = lines.map(|l| l.split('\t').collect()).collect();
    let (mut equal, mut different, mut error) = (0, 0, 0);
    for (i, r) in results.iter().enumerate() {
        let word = match rec.get(i) {
            Some(f) if f.len() == 4 && f[0] == (i + 1).to_string() => {
                if !r.ok || f[1] != "ok" {
                    "error"
                } else if f[2] == r.n.to_string() && f[3] == r.hash {
                    "equal"
                } else {
                    "different"
                }
            }
            _ => "different",
        };
        match word {
            "equal" => equal += 1,
            "error" => error += 1,
            _ => different += 1,
        }
        if !all || word != "equal" {
            say("search", &format!("{}\t{word}", i + 1));
        }
    }
    if rec.len() != results.len() {
        different += rec.len().abs_diff(results.len());
    }
    say(
        "search",
        &format!(
            "queries {} with-results {with} equal {equal} different {different} error {error}",
            results.len()
        ),
    );
    if error > 0 {
        FAILED.store(true, Ordering::SeqCst);
    }
    Ok(())
}

// ---- files ----------------------------------------------------------------------

/// Reads a regular file, never through a link (O_NOFOLLOW), up to `cap`.
fn read_capped(p: &Path, cap: u64) -> Result<Vec<u8>, &'static str> {
    let meta = fs::symlink_metadata(p).map_err(|_| "unreadable")?;
    if meta.file_type().is_symlink() {
        return Err("link");
    }
    if !meta.file_type().is_file() {
        return Err("not-a-file");
    }
    if meta.len() > cap {
        return Err("oversize");
    }
    let f = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc_o_nofollow())
        .open(p)
        .map_err(|_| "unreadable")?;
    let mut b = Vec::with_capacity(meta.len() as usize);
    f.take(cap + 1)
        .read_to_end(&mut b)
        .map_err(|_| "unreadable")?;
    if b.len() as u64 > cap {
        return Err("oversize");
    }
    Ok(b)
}

/// What one directory entry is, by `symlink_metadata` (never followed).
enum Entry {
    Dir,
    File,
    Link,
    Other,
}

/// One listing of `dir`: its entries sorted by name, each with its kind.
/// `Ok(None)` when `dir` does not exist.
fn list(dir: &Path) -> Result<Option<Vec<(String, Entry)>>, String> {
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("list".into()),
    };
    let mut out = Vec::new();
    for e in rd {
        let e = e.map_err(|_| "list-entry".to_string())?;
        let name = e.file_name().to_string_lossy().into_owned();
        let kind = match fs::symlink_metadata(e.path()) {
            Ok(m) if m.file_type().is_symlink() => Entry::Link,
            Ok(m) if m.file_type().is_dir() => Entry::Dir,
            Ok(m) if m.file_type().is_file() => Entry::File,
            _ => Entry::Other,
        };
        out.push((name, kind));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Some(out))
}

/// Whether every component from `base` down to `base/rel...` is a real
/// directory: `Some(true)` all real, `Some(false)` one is absent, `None` one
/// is a link or not a directory (`memory_files/partition.rs`'s
/// `real_directory_state`: a link on the way refuses the partition).
fn real_dirs(base: &Path, rel: &[&str]) -> Option<bool> {
    let mut p = base.to_path_buf();
    for r in rel {
        p.push(r);
        match fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_dir() => {}
            Ok(_) => return None,
            Err(_) => return Some(false),
        }
    }
    Some(true)
}

fn is_lower_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_owner_key(s: &str) -> bool {
    is_lower_hex(s, OWNER_KEY_HEX)
}

/// `memory_files::is_partition_key`: `home-<32 hex>` or `proj-<32 hex>-<16 hex>`.
fn is_partition_key(s: &str) -> bool {
    if let Some(o) = s.strip_prefix("home-") {
        return is_owner_key(o);
    }
    if let Some(rest) = s.strip_prefix("proj-")
        && let Some((o, d)) = rest.split_once('-')
    {
        return is_owner_key(o) && is_lower_hex(d, DIR_KEY_HEX);
    }
    false
}

/// `NoteId::parse` / `ProjectId::parse`: the canonical hyphenated lowercase
/// UUID spelling only.
fn is_canonical_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(c),
        })
}

/// `memory_files::owner_key`: the first 32 characters of `sha256_hex(owner_id)`.
fn owner_key(owner_id: &str) -> String {
    sha256_hex(owner_id.as_bytes())[..OWNER_KEY_HEX].to_string()
}

/// `memory_files::project_key`: the first 16 characters of
/// `sha256_hex(project_id)`, over its canonical spelling.
fn project_key(project_id: &str) -> String {
    sha256_hex(project_id.as_bytes())[..DIR_KEY_HEX].to_string()
}

/// `Note::id_from_file_name` (`memory_files/note.rs`): `<uuid>--<slug>.md`.
fn id_from_file_name(name: &str) -> Option<&str> {
    let stem = name.strip_suffix(".md")?;
    let (id, _) = stem.split_once("--")?;
    is_canonical_uuid(id).then_some(id)
}

/// `dirstat::is_note_name`: `*.md`, not hidden.
fn is_note_name(name: &str) -> bool {
    !name.starts_with('.') && name.ends_with(".md")
}

/// `projects/*/project.json` as `store/projects.rs`'s `list` reads it: each
/// folder's name, its id (canonical), version, and whether `list` returns
/// it. The recorded `root` is only carried to `--out` by the inventory,
/// never opened.
struct ProjectEntry {
    name: String,
    id: Option<String>,
    version: Option<u64>,
    root_for_out: Option<String>,
    /// `read_project_dir` reads it: its `project.json` a regular file of one
    /// link, never a link, that parses as a `ProjectFile` — `version` 1, a
    /// canonical `id`, an absolute `root`, a `created_at` string, and
    /// `previous_root` and `previous_name` each a string, null or absent.
    readable: bool,
    /// Another readable folder carries the same id: `list` leaves both out.
    shared_id: bool,
}

impl ProjectEntry {
    /// What `list` returns: readable, and the only folder with its id.
    fn listed(&self) -> bool {
        self.readable && !self.shared_id
    }

    fn word(&self) -> &'static str {
        match (self.readable, self.shared_id) {
            (true, false) => "listed",
            (true, true) => "shared-id",
            _ => "unreadable",
        }
    }
}

struct Projects {
    entries: Vec<ProjectEntry>,
    /// Dot-prefixed names, which `list` passes over (a folder `register` is
    /// still building).
    hidden: usize,
    link_skipped: usize,
    other: usize,
}

/// One folder's `project.json`, by `read_project_dir`'s rule.
fn read_project_file(dir: &Path, e: &mut ProjectEntry) {
    let f = dir.join("project.json");
    let one_link = fs::symlink_metadata(&f).is_ok_and(|m| m.nlink() == 1);
    let Ok(b) = read_capped(&f, SMALL_READ_CAP) else {
        return;
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b) else {
        return;
    };
    let string_or_absent = |k: &str| {
        matches!(
            v.get(k),
            None | Some(serde_json::Value::Null | serde_json::Value::String(_))
        )
    };
    e.version = v.get("version").and_then(serde_json::Value::as_u64);
    e.id = v
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|s| is_canonical_uuid(s))
        .map(str::to_string);
    e.root_for_out = v
        .get("root")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    e.readable = one_link
        && e.version == Some(1)
        && e.id.is_some()
        && e.root_for_out
            .as_deref()
            .is_some_and(|r| Path::new(r).is_absolute())
        && v.get("created_at")
            .is_some_and(serde_json::Value::is_string)
        && string_or_absent("previous_root")
        && string_or_absent("previous_name");
}

fn read_projects(store: &Path) -> Result<Projects, String> {
    let mut p = Projects {
        entries: Vec::new(),
        hidden: 0,
        link_skipped: 0,
        other: 0,
    };
    let dir = store.join("projects");
    match real_dirs(store, &["projects"]) {
        Some(true) => {}
        Some(false) => return Ok(p),
        None => {
            p.link_skipped += 1;
            return Ok(p);
        }
    }
    for (name, kind) in list(&dir)?.unwrap_or_default() {
        if name.starts_with('.') {
            p.hidden += 1;
            continue;
        }
        match kind {
            Entry::Link => p.link_skipped += 1,
            Entry::Dir => {
                let mut e = ProjectEntry {
                    name: name.clone(),
                    id: None,
                    version: None,
                    root_for_out: None,
                    readable: false,
                    shared_id: false,
                };
                read_project_file(&dir.join(&name), &mut e);
                p.entries.push(e);
            }
            _ => p.other += 1,
        }
    }
    // Two readable folders carrying one id (a copied folder): `list` opens
    // neither, so neither locates a partition.
    let mut ids: BTreeMap<String, usize> = BTreeMap::new();
    for id in p
        .entries
        .iter()
        .filter(|e| e.readable)
        .filter_map(|e| e.id.clone())
    {
        *ids.entry(id).or_default() += 1;
    }
    for e in &mut p.entries {
        e.shared_id = e.readable
            && e.id
                .as_ref()
                .is_some_and(|id| ids.get(id).is_some_and(|n| *n > 1));
    }
    Ok(p)
}

// ---- snapshots ------------------------------------------------------------------

#[derive(Default, Clone, Copy)]
struct NoteCounts {
    notes: usize,
    matched: usize,
    mismatched: usize,
    mismatched_pending: usize,
    missing: usize,
    /// Recorded notes whose folder the copy does not give: an orphan
    /// snapshot (an earlier build's with no project id, R22, or one whose id
    /// no `projects/*/project.json` records) or a notes folder reached
    /// through a link. Neither the daemon nor the probe reads those files,
    /// so they are not `missing`.
    unlocated: usize,
    duplicate: usize,
    oversize: usize,
    unreadable: usize,
    unrecorded: usize,
    unnamed: usize,
    forgotten_files: usize,
    link_skipped: usize,
}

impl NoteCounts {
    fn add(&mut self, o: &NoteCounts) {
        self.notes += o.notes;
        self.matched += o.matched;
        self.mismatched += o.mismatched;
        self.mismatched_pending += o.mismatched_pending;
        self.missing += o.missing;
        self.unlocated += o.unlocated;
        self.duplicate += o.duplicate;
        self.oversize += o.oversize;
        self.unreadable += o.unreadable;
        self.unrecorded += o.unrecorded;
        self.unnamed += o.unnamed;
        self.forgotten_files += o.forgotten_files;
        self.link_skipped += o.link_skipped;
    }

    fn fields(&self) -> String {
        format!(
            "notes={}\tmatched={}\tmismatched={}\tmismatched-pending={}\tmissing={}\tunlocated={}\tduplicate={}\toversize={}\tunreadable={}\tunrecorded={}\tunnamed={}\tforgotten-files={}\tlink-skipped={}",
            self.notes,
            self.matched,
            self.mismatched,
            self.mismatched_pending,
            self.missing,
            self.unlocated,
            self.duplicate,
            self.oversize,
            self.unreadable,
            self.unrecorded,
            self.unnamed,
            self.forgotten_files,
            self.link_skipped
        )
    }
}

/// One snapshot file as the probe reads it: a `serde_json::Value`, not the
/// daemon's `deny_unknown_fields` struct (`memory_files/snapshot.rs`,
/// `ControlSnapshot`), so a field it does not know never hides the rest.
struct Snap {
    file_stem: String,
    v: serde_json::Value,
}

impl Snap {
    fn s(&self, k: &str) -> Option<&str> {
        self.v.get(k).and_then(serde_json::Value::as_str)
    }
}

/// What [`read_snapshots`] found: each file `Ok(snapshot)` or `Err(stem)` when
/// it does not parse; the links skipped; whether the control dir is there.
type Snapshots = (Vec<Result<Snap, String>>, usize, bool);

/// The control dir's snapshot files: `state/memory-control/*.json`, hidden
/// names left out (`partition.rs`'s `list_snapshots`), links skipped and
/// counted.
fn read_snapshots(store: &Path) -> Result<Snapshots, String> {
    let mut link_skipped = 0;
    let dir = store.join("state").join("memory-control");
    match real_dirs(store, &["state", "memory-control"]) {
        Some(true) => {}
        Some(false) => return Ok((Vec::new(), 0, false)),
        None => return Ok((Vec::new(), 1, false)),
    }
    let mut out = Vec::new();
    for (name, kind) in list(&dir)?.unwrap_or_default() {
        if name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }
        let stem = name.trim_end_matches(".json").to_string();
        match kind {
            Entry::Link => link_skipped += 1,
            Entry::File => {
                let parsed = read_capped(&dir.join(&name), SMALL_READ_CAP)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                    .filter(serde_json::Value::is_object);
                out.push(match parsed {
                    Some(v) => Ok(Snap { file_stem: stem, v }),
                    None => Err(stem),
                });
            }
            _ => {}
        }
    }
    Ok((out, link_skipped, true))
}

fn key_or_invalid(s: &str) -> &str {
    if is_partition_key(s) {
        s
    } else {
        "invalid-key"
    }
}

/// Re-derives the partition key and every recorded note's revision with this
/// build's `sha256_hex`, and compares them with what the snapshot records.
/// The notes folder comes from the COPY, never from `root`: home →
/// `<store>/memory/owners/<owner key>/notes/`; project → the
/// `<store>/projects/<name>/` whose `project.json` id is the snapshot's
/// `project_id` → `.../memory/owners/<owner key>/notes/` (no match: `orphan`).
fn check_snapshots(store: &Path, detail: &mut Detail, knob: &Option<String>) -> Result<(), String> {
    maybe_panic(knob, "snapshots", store);
    let (snaps, link_skipped, present) = read_snapshots(store)?;
    let projects = read_projects(store)?;
    if !present {
        say(
            "snapshots",
            if link_skipped > 0 {
                "control-dir link-skipped"
            } else {
                "control-dir absent"
            },
        );
    }
    let mut total = NoteCounts::default();
    let (mut unparsable, mut key_different, mut key_absent, mut orphan) = (0, 0, 0, 0);
    for (i, s) in snaps.iter().enumerate() {
        let idx = i + 1;
        let s = match s {
            Ok(s) => s,
            Err(stem) => {
                unparsable += 1;
                say(
                    "snapshots",
                    &format!("{idx}\t{}\tunparsable", key_or_invalid(stem)),
                );
                continue;
            }
        };
        let pkey = s.s("partition_key").unwrap_or("");
        let scope = match s.s("scope") {
            Some("home") => "home",
            Some("project") => "project",
            _ => "other",
        };
        let schema = if s.v.get("schema").and_then(serde_json::Value::as_u64) == Some(1) {
            "1"
        } else {
            "other"
        };
        let derived_ok = s.s("owner_id").map(owner_key);
        let recorded_ok = s.s("owner_key").unwrap_or("");
        let project_id = s.s("project_id").filter(|p| is_canonical_uuid(p));
        let key = match (scope, derived_ok.as_deref()) {
            ("home", Some(ok)) => {
                if recorded_ok == ok && pkey == format!("home-{ok}") && s.file_stem == pkey {
                    "equal"
                } else {
                    "different"
                }
            }
            ("project", Some(ok)) => match project_id {
                None => "absent",
                Some(pid) => {
                    if recorded_ok == ok
                        && pkey == format!("proj-{ok}-{}", project_key(pid))
                        && s.file_stem == pkey
                    {
                        "equal"
                    } else {
                        "different"
                    }
                }
            },
            _ => "different",
        };
        match key {
            "different" => key_different += 1,
            "absent" => key_absent += 1,
            _ => {}
        }
        // The owner directory the daemon would open: the owner key in the
        // partition key (the file's own name), as `Partition::open_readonly`
        // derives the snapshot path from its directory key.
        let dir_ok = if is_partition_key(pkey) {
            pkey.split('-').nth(1).map(str::to_string)
        } else {
            None
        };
        let folder: Option<(&str, PathBuf, Vec<String>)> = match (scope, dir_ok) {
            ("home", Some(ok)) => Some((
                "home",
                store.to_path_buf(),
                vec!["memory".into(), "owners".into(), ok],
            )),
            ("project", Some(ok)) => project_id
                .and_then(|pid| {
                    projects
                        .entries
                        .iter()
                        .find(|e| e.listed() && e.id.as_deref() == Some(pid))
                })
                .map(|e| {
                    (
                        "project",
                        store.to_path_buf(),
                        vec![
                            "projects".into(),
                            e.name.clone(),
                            "memory".into(),
                            "owners".into(),
                            ok,
                        ],
                    )
                }),
            _ => None,
        };
        let notes_obj = s.v.get("notes").and_then(serde_json::Value::as_object);
        let tombstones: BTreeSet<String> =
            s.v.get("tombstones")
                .and_then(serde_json::Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|t| {
                            t.get("id")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string)
                        })
                        .collect()
                })
                .unwrap_or_default();
        let pending = s.v.get("pending").filter(|p| p.is_object());
        let pending_id = pending
            .and_then(|p| p.get("id"))
            .and_then(serde_json::Value::as_str);
        let pending_to = pending
            .and_then(|p| p.get("to_rev"))
            .and_then(serde_json::Value::as_str);

        let mut nc = NoteCounts::default();
        let mut notes_dir_word = "absent";
        let folder_word = match &folder {
            Some((w, _, _)) => *w,
            None => {
                orphan += 1;
                "orphan"
            }
        };
        let mut files: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut notes_dir = PathBuf::new();
        if let Some((_, base, rel)) = &folder {
            let rel_refs: Vec<&str> = rel.iter().map(String::as_str).collect();
            let owner_root = rel.iter().fold(base.clone(), |p, r| p.join(r));
            // `Partition::open_inner`'s components: each that exists a real
            // directory, `.history` beside `notes` included; a link or a
            // file among them refuses the partition.
            let path_state = match real_dirs(base, &rel_refs) {
                Some(true) => match (
                    real_dirs(&owner_root, &["notes"]),
                    real_dirs(&owner_root, &[".history"]),
                ) {
                    (None, _) | (_, None) => None,
                    (notes, _) => notes,
                },
                other => other,
            };
            match path_state {
                Some(true) => {
                    notes_dir_word = "present";
                    notes_dir = owner_root.join("notes");
                    for (name, kind) in list(&notes_dir)?.unwrap_or_default() {
                        if !is_note_name(&name) {
                            continue;
                        }
                        match kind {
                            Entry::Link => nc.link_skipped += 1,
                            Entry::File => match id_from_file_name(&name) {
                                Some(id) => {
                                    files.entry(id.to_string()).or_default().push(name.clone())
                                }
                                None => nc.unnamed += 1,
                            },
                            _ => {}
                        }
                    }
                }
                Some(false) => notes_dir_word = "absent",
                None => notes_dir_word = "link-skipped",
            }
        }
        // `missing` means the folder was listed and the file is not in it.
        let locatable = folder.is_some() && notes_dir_word != "link-skipped";
        if let Some(notes) = notes_obj {
            for (id, rec) in notes {
                nc.notes += 1;
                if !locatable {
                    nc.unlocated += 1;
                    detail.line(&format!("snapshots\t{idx}\tnote\t{id}\tunlocated"))?;
                    continue;
                }
                let revision = rec
                    .get("revision")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let word = match files.get(id).map(Vec::as_slice) {
                    None | Some([]) => {
                        nc.missing += 1;
                        "missing"
                    }
                    Some([one]) => match read_capped(&notes_dir.join(one), NOTE_READ_CAP) {
                        Err("oversize") => {
                            nc.oversize += 1;
                            "oversize"
                        }
                        Err(_) => {
                            nc.unreadable += 1;
                            "unreadable"
                        }
                        Ok(bytes) => {
                            let h = sha256_hex(&bytes);
                            if h == revision {
                                nc.matched += 1;
                                "matched"
                            } else if pending_id == Some(id.as_str())
                                && pending_to == Some(h.as_str())
                            {
                                nc.mismatched_pending += 1;
                                "mismatched-pending"
                            } else {
                                nc.mismatched += 1;
                                "mismatched"
                            }
                        }
                    },
                    Some(_) => {
                        nc.duplicate += 1;
                        "duplicate"
                    }
                };
                detail.line(&format!("snapshots\t{idx}\tnote\t{id}\t{word}"))?;
            }
        }
        for (id, names) in &files {
            if notes_obj.map(|n| n.contains_key(id)).unwrap_or(false) {
                continue;
            }
            if tombstones.contains(id) {
                nc.forgotten_files += names.len();
            } else {
                nc.unrecorded += names.len();
            }
        }
        total.add(&nc);
        say(
            "snapshots",
            &format!(
                "{idx}\t{}\t{scope}\tschema={schema}\tkey={key}\tfolder={folder_word}\tnotes-dir={notes_dir_word}\t{}\ttombstones={}\tpending={}",
                key_or_invalid(pkey),
                nc.fields(),
                tombstones.len(),
                usize::from(pending.is_some())
            ),
        );
    }
    say(
        "snapshots",
        &format!(
            "total\tcount={}\tunparsable={unparsable}\tlink-skipped={link_skipped}\tkey-different={key_different}\tkey-absent={key_absent}\torphan={orphan}\t{}",
            snaps.len(),
            total.fields()
        ),
    );
    Ok(())
}

// ---- inventory ------------------------------------------------------------------

/// E8 step 4 (R10): the expected partition set, independent of both doctors —
/// `projects/*/project.json`, every owner folder under `memory/owners/` and
/// `projects/*/memory/owners/`, every snapshot. Counts and partition keys on
/// stdout; names, ids and recorded roots to `--out`.
fn check_inventory(store: &Path, detail: &mut Detail, knob: &Option<String>) -> Result<(), String> {
    maybe_panic(knob, "inventory", store);
    let projects = read_projects(store)?;
    let listed = projects.entries.iter().filter(|e| e.listed()).count();
    let shared = projects.entries.iter().filter(|e| e.shared_id).count();
    say(
        "inventory",
        &format!(
            "projects folders={} readable={listed} unreadable={} shared-id={shared} hidden={} link-skipped={} other={}",
            projects.entries.len(),
            projects.entries.len() - listed - shared,
            projects.hidden,
            projects.link_skipped,
            projects.other
        ),
    );
    for (i, e) in projects.entries.iter().enumerate() {
        detail.line(&format!(
            "inventory\tproject\t{}\t{}\t{}\t{}\t{}\t{}",
            i + 1,
            e.word(),
            e.name,
            e.id.as_deref().unwrap_or("-"),
            e.version
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".into()),
            e.root_for_out.as_deref().unwrap_or("-")
        ))?;
    }
    // Owner folders: (partition key, note files) — a project folder's key
    // from its project.json id; an unreadable one has none (`unkeyed`).
    let mut folders: BTreeMap<String, usize> = BTreeMap::new();
    let (mut unkeyed, mut other, mut links) = (0usize, 0usize, 0usize);
    let mut owner_dirs = |base: &Path,
                          rel: &[&str],
                          key_of: &dyn Fn(&str) -> Option<String>|
     -> Result<(), String> {
        match real_dirs(base, rel) {
            Some(true) => {}
            Some(false) => return Ok(()),
            None => {
                links += 1;
                return Ok(());
            }
        }
        let owners = rel.iter().fold(base.to_path_buf(), |p, r| p.join(r));
        for (name, kind) in list(&owners)?.unwrap_or_default() {
            match kind {
                Entry::Dir if is_owner_key(&name) => {
                    let n = match real_dirs(&owners, &[name.as_str(), "notes"]) {
                        Some(true) => list(&owners.join(&name).join("notes"))?
                            .unwrap_or_default()
                            .iter()
                            .filter(|(n, k)| is_note_name(n) && matches!(k, Entry::File))
                            .count(),
                        _ => 0,
                    };
                    match key_of(&name) {
                        Some(k) => {
                            folders.insert(k, n);
                        }
                        None => unkeyed += 1,
                    }
                }
                Entry::Link => links += 1,
                _ => other += 1,
            }
        }
        Ok(())
    };
    owner_dirs(store, &["memory", "owners"], &|ok| {
        Some(format!("home-{ok}"))
    })?;
    for e in &projects.entries {
        let id = if e.listed() { e.id.clone() } else { None };
        owner_dirs(
            store,
            &["projects", e.name.as_str(), "memory", "owners"],
            &|ok| {
                id.as_deref()
                    .map(|pid| format!("proj-{ok}-{}", project_key(pid)))
            },
        )?;
    }
    say(
        "inventory",
        &format!(
            "owner-folders keyed={} unkeyed={unkeyed} link-skipped={links} other={other}",
            folders.len()
        ),
    );
    // Snapshots: (partition key, scope, notes, tombstones, project id).
    let (snaps, snap_links, _) = read_snapshots(store)?;
    let mut snap_keys: BTreeMap<String, String> = BTreeMap::new();
    let mut unparsable = 0;
    for (i, s) in snaps.iter().enumerate() {
        let idx = i + 1;
        match s {
            Err(stem) => {
                unparsable += 1;
                say(
                    "inventory",
                    &format!("snapshot\t{idx}\t{}\tunparsable", key_or_invalid(stem)),
                );
            }
            Ok(s) => {
                let pkey = key_or_invalid(s.s("partition_key").unwrap_or(""));
                let scope = match s.s("scope") {
                    Some("home") => "home",
                    Some("project") => "project",
                    _ => "other",
                };
                let notes =
                    s.v.get("notes")
                        .and_then(serde_json::Value::as_object)
                        .map_or(0, |m| m.len());
                let tomb =
                    s.v.get("tombstones")
                        .and_then(serde_json::Value::as_array)
                        .map_or(0, Vec::len);
                let pid = match (scope, s.s("project_id").filter(|p| is_canonical_uuid(p))) {
                    ("project", Some(_)) => "present",
                    ("project", None) => "absent",
                    _ => "none",
                };
                say(
                    "inventory",
                    &format!(
                        "snapshot\t{idx}\t{pkey}\t{scope}\tnotes={notes}\ttombstones={tomb}\tproject-id={pid}"
                    ),
                );
                if pkey != "invalid-key" {
                    snap_keys.insert(pkey.to_string(), scope.to_string());
                }
                detail.line(&format!(
                    "inventory\tsnapshot\t{idx}\t{}.json\t{pkey}",
                    s.file_stem
                ))?;
            }
        }
    }
    say(
        "inventory",
        &format!(
            "snapshots count={} unparsable={unparsable} link-skipped={snap_links}",
            snaps.len()
        ),
    );
    let all: BTreeSet<&String> = folders.keys().chain(snap_keys.keys()).collect();
    let (mut both, mut folder_only, mut snap_only) = (0, 0, 0);
    for k in &all {
        let f = folders.get(*k);
        let s = snap_keys.contains_key(*k);
        match (f.is_some(), s) {
            (true, true) => both += 1,
            (true, false) => folder_only += 1,
            _ => snap_only += 1,
        }
        say(
            "inventory",
            &format!(
                "partition\t{k}\tfolder={}\tsnapshot={}\tnote-files={}",
                if f.is_some() { "yes" } else { "no" },
                if s { "yes" } else { "no" },
                f.map(|n| n.to_string()).unwrap_or_else(|| "-".into())
            ),
        );
    }
    say(
        "inventory",
        &format!(
            "partitions total={} both={both} folder-only={folder_only} snapshot-only={snap_only}",
            all.len()
        ),
    );
    Ok(())
}

// ---- main -----------------------------------------------------------------------

fn main() {
    std::panic::set_hook(Box::new(|_| {
        let _ = writeln!(std::io::stderr(), "{PANIC_LINE}");
    }));
    // The known answer first, so a digest API that compiles but spells
    // differently stops the run before any input is read.
    if sha256_hex(b"abc") != KAT_ABC {
        refuse("sha2-known-answer");
    }
    let mut a = Args::parse();
    let knob = selftest_knob();
    guard(&mut a);
    let p = |f: &str| a.get(f).map(Path::to_path_buf);
    let mut detail = match a.get("--out") {
        Some(o) => Detail::create(o),
        None => Detail::none(),
    };
    match a.cmd.as_str() {
        "version" => {
            say("e8probe", &format!("sqlite {}", rusqlite::version()));
            say("e8probe", "sha2-known-answer ok");
        }
        "db" => {
            let (db, s3) = (p("--db").unwrap_or_default(), p("--step3"));
            run_check("db", &mut detail, |d| {
                check_db(&db, s3.as_deref(), d, &knob)
            });
        }
        "index" => {
            let c = p("--cache").unwrap_or_default();
            run_check("index", &mut detail, |d| check_index(&c, d, &knob));
        }
        "fts-rw" => {
            let rw = p("--cache-rw").unwrap_or_default();
            run_check("fts-rw", &mut detail, |d| check_fts_rw(&rw, d, &knob));
        }
        "search" => {
            let (c, r) = (
                p("--cache").unwrap_or_default(),
                p("--record").unwrap_or_default(),
            );
            let compare = a.mode.as_deref() == Some("compare");
            run_check("search", &mut detail, |_| {
                check_search(&c, &r, compare, false, &knob)
            });
        }
        "snapshots" => {
            let s = p("--store").unwrap_or_default();
            run_check("snapshots", &mut detail, |d| check_snapshots(&s, d, &knob));
        }
        "inventory" => {
            let s = p("--store").unwrap_or_default();
            run_check("inventory", &mut detail, |d| check_inventory(&s, d, &knob));
        }
        "all" => {
            let store = p("--store").unwrap_or_default();
            // `<store>/state/openalpaca.db`, put there and checked by the guard.
            let db = p("--db").unwrap_or_default();
            let (ro, rw) = (
                p("--cache-ro").unwrap_or_default(),
                p("--cache-rw").unwrap_or_default(),
            );
            let (rec, s3) = (p("--record").unwrap_or_default(), p("--step3"));
            say("e8probe", &format!("all sqlite {}", rusqlite::version()));
            run_check("db", &mut detail, |d| {
                check_db(&db, s3.as_deref(), d, &knob)
            });
            if exists_nofollow(&ro) {
                run_check("index", &mut detail, |d| check_index(&ro, d, &knob));
                if exists_nofollow(&rw) {
                    run_check("fts-rw", &mut detail, |d| check_fts_rw(&rw, d, &knob));
                } else {
                    say("fts-rw", "absent");
                }
                let compare = exists_nofollow(&rec);
                run_check("search", &mut detail, |_| {
                    check_search(&ro, &rec, compare, true, &knob)
                });
            } else {
                say("index", "absent");
                say("fts-rw", "absent");
                say("search", "absent");
            }
            run_check("snapshots", &mut detail, |d| {
                check_snapshots(&store, d, &knob)
            });
        }
        _ => usage(),
    }
    std::process::exit(if FAILED.load(Ordering::SeqCst) { 1 } else { 0 });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The known answer this build's sha2 gives, in the one spelling
    /// OpenAlpaca writes — the same test on both variants.
    #[test]
    fn sha256_known_answers() {
        assert_eq!(sha256_hex(b"abc"), KAT_ABC);
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn keys_are_the_daemons() {
        // owner_key / project_key are prefixes of sha256_hex.
        assert_eq!(owner_key("abc"), &KAT_ABC[..32]);
        assert_eq!(project_key("abc"), &KAT_ABC[..16]);
        assert!(is_partition_key(&format!("home-{}", owner_key("x"))));
        assert!(is_partition_key(&format!(
            "proj-{}-{}",
            owner_key("x"),
            project_key("y")
        )));
        assert!(!is_partition_key("home-ABC"));
    }

    #[test]
    fn names_and_ids() {
        assert!(is_canonical_uuid("0f8fad5b-d9cb-469f-a165-70867728950e"));
        assert!(!is_canonical_uuid("0F8FAD5B-D9CB-469F-A165-70867728950E"));
        assert_eq!(
            id_from_file_name("0f8fad5b-d9cb-469f-a165-70867728950e--a-title.md"),
            Some("0f8fad5b-d9cb-469f-a165-70867728950e")
        );
        assert_eq!(id_from_file_name("notes.md"), None);
        assert!(!is_note_name(".x.md"));
        assert_eq!(first_word("a bc-def"), Some("bc".to_string()));
        assert_eq!(first_word("x y"), None);
    }

    #[test]
    fn uri_is_read_only_and_encoded() {
        assert_eq!(
            uri_ro(Path::new("/a b/c?d#e%f")),
            "file:/a%20b/c%3Fd%23e%25f?mode=ro"
        );
        assert!(!uri_ro(Path::new("/x")).contains("immutable"));
    }

    #[test]
    fn openalpaca_components() {
        assert!(has_openalpaca_component(Path::new(
            "/u/x/.openalpaca/state"
        )));
        assert!(!has_openalpaca_component(Path::new("/u/x/openalpaca-copy")));
        assert!(has_openalpaca_component(Path::new("/u/x/.OpenAlpaca")));
        assert!(has_openalpaca_component(Path::new("/u/.OPENALPACA/state")));
        assert!(!has_openalpaca_component(Path::new(
            "/u/-Users-x-dev-OpenAlpaca/state"
        )));
    }
}
