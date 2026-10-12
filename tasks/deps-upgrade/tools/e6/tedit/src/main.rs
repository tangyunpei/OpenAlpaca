//! tedit — the E6 comparator for `toml_edit`: 0.25.13 (the lock today) against
//! 0.25.17 (what P04's refresh takes). One source, two manifests (`old/`,
//! `new/`) that differ in the `toml_edit` line alone; the typed side is `toml`
//! 0.9.12 in both, as `edit.rs` has it today — P29's `toml` move is `tomlcmp`'s
//! subject, not this tool's.
//!
//! Cargo refuses two 0.25.x `toml_edit`s in one graph, so the pair runs as two
//! binaries in two steps:
//!
//!   tedit record  --out <file>  <input>...     # the OLD binary: a record, to a file you name
//!   tedit compare --from <file> <input>...     # the NEW binary: one line per input
//!
//! `compare` prints `<index>\t<word>` and exits 0 — `equal`, `different`,
//! `parse-error-old`, `parse-error-new`, `parse-error-both` or `unreadable`;
//! with `--summary` counts only; with `--roundtrip` a word per input on the
//! A08 round-trip defect (`reparse_equal=false`): `roundtrip-ok`,
//! `defect-shared`, `defect-old-only`, `defect-new-only` or `not-applicable`.
//! The record file holds, per input, its index, a status word, a SHA-256 and two
//! small counts — never a value, a key or a path. Nothing else is written.
//!
//! What the record fingerprints, per input (the September tool's facets, nothing
//! added): whether `DocumentMut::to_string` gives the input back byte for byte;
//! a fresh-encoded render of the document; its `fmt()` render; and, for each of
//! six seeded mutations of the typed value (the September `mutate`, seeds
//! `s * 7919 + 17`), the text `apply_delta` writes and whether `toml` 0.9.12
//! re-parses that text to the mutated value.

use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, Item, TableLike, Value};

// ---- from the September tool, tedit-0.25.15/src/main.rs:4-22, rustfmt-formatted
fn fresh_value(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::from(s.value().as_str()),
        Value::Integer(i) => Value::from(*i.value()),
        Value::Float(f) => Value::from(*f.value()),
        Value::Boolean(b) => Value::from(*b.value()),
        Value::Datetime(d) => Value::from(*d.value()),
        Value::Array(a) => {
            let mut n = toml_edit::Array::new();
            for x in a.iter() {
                n.push(fresh_value(x));
            }
            Value::Array(n)
        }
        Value::InlineTable(t) => {
            let mut n = toml_edit::InlineTable::new();
            for (k, x) in t.iter() {
                n.insert(k, fresh_value(x));
            }
            Value::InlineTable(n)
        }
    }
}
fn fresh_item(i: &Item) -> Item {
    match i {
        Item::None => Item::None,
        Item::Value(v) => Item::Value(fresh_value(v)),
        Item::Table(t) => {
            let mut n = toml_edit::Table::new();
            n.set_implicit(t.is_implicit());
            for (k, x) in t.iter() {
                n.insert(k, fresh_item(x));
            }
            Item::Table(n)
        }
        Item::ArrayOfTables(a) => {
            let mut n = toml_edit::ArrayOfTables::new();
            for t in a.iter() {
                if let Item::Table(tt) = fresh_item(&Item::Table(t.clone())) {
                    n.push(tt);
                }
            }
            Item::ArrayOfTables(n)
        }
    }
}
// ---------------------------------------------------------------------------

// ---- verbatim from crates/openalpaca_llm/src/config/llm_config/edit.rs:74-184
/// Rewrite `target` so it says what `after` says, touching only the keys whose
/// value actually moved between `before` and `after`.
///
/// `rendered` is `after` serialised, and supplies the replacement items.
///
/// Works the same on a `[section]` and on a hand-written `providers = { … }`:
/// `TableLike` covers both, and an inline table converts a table item to an
/// inline one on the way in (`Item::into_value`), so the owner's formatting
/// decides the shape rather than this code.
fn apply_delta(
    target: &mut dyn TableLike,
    before: &toml::Table,
    after: &toml::Table,
    rendered: &dyn TableLike,
) {
    for (key, after_value) in after.iter() {
        let before_value = before.get(key);
        if before_value == Some(after_value) {
            // Untouched by this write — and therefore untouched in the file,
            // comments, spacing, position and all.
            continue;
        }

        let Some(rendered_item) = rendered.get(key) else {
            continue;
        };

        // A table on both sides: descend rather than replace, so a change to
        // one key inside `[providers.ollama]` does not rewrite the block.
        if let toml::Value::Table(after_sub) = after_value
            && let Some(rendered_sub) = rendered_item.as_table_like()
        {
            // A block the file does not have yet starts as an empty table and
            // is filled key by key below, so it arrives with ordinary
            // formatting instead of the source document's positions.
            if target.get(key).and_then(Item::as_table_like).is_none() {
                target.insert(key, Item::Table(toml_edit::Table::new()));
            }
            let empty = toml::Table::new();
            let before_sub = match before_value {
                Some(toml::Value::Table(t)) => t,
                _ => &empty,
            };
            if let Some(target_sub) = target.get_mut(key).and_then(Item::as_table_like_mut) {
                apply_delta(target_sub, before_sub, after_sub, rendered_sub);
                continue;
            }
        }

        set_preserving_decor(target, key, rendered_item.clone());
    }

    // A key this crate's types knew about and no longer emit was removed on
    // purpose (a deleted API key, a cleared override). A key they never knew
    // about is the owner's, and stays.
    for key in before.keys() {
        if !after.contains_key(key) {
            target.remove(key);
        }
    }
}

/// Put `new_item` at `key` without disturbing how the file writes that key.
///
/// `TableLike::insert` re-formats an existing key, which drops the comment
/// lines standing above it — exactly what this module exists to keep. So an
/// existing entry is replaced through `get_mut`, and the shape the file already
/// uses for that key is what it keeps: a value stays a value, carrying its own
/// spacing and trailing comment, and a `[[section]]` array of tables stays one
/// rather than collapsing into an inline `[{ … }]` (`Item::into_value` would
/// happily convert it).
fn set_preserving_decor(target: &mut dyn TableLike, key: &str, mut new_item: Item) {
    let Some(existing) = target.get_mut(key) else {
        target.insert(key, new_item);
        return;
    };

    if existing.is_value() {
        new_item = match new_item.into_value() {
            Ok(value) => Item::Value(value),
            Err(item) => item,
        };
    }
    carry_decor(existing, &mut new_item);
    *existing = new_item;
}

/// Move the formatting of what was in the file onto what replaces it.
///
/// For a plain value that is its spacing and any trailing comment. For a
/// `[[section]]` array it is the comment standing above each entry — which
/// lives in that entry's own decor, so a changed key list would otherwise come
/// back with the owner's notes stripped. Matched up by position, because
/// formatting is formatting: a wrong guess is cosmetic, and there is nothing
/// else to match on.
fn carry_decor(old: &Item, new: &mut Item) {
    match (old, new) {
        (Item::Value(old), Item::Value(new)) => *new.decor_mut() = old.decor().clone(),
        (Item::Table(old), Item::Table(new)) => {
            *new.decor_mut() = old.decor().clone();
            new.set_position(old.position());
        }
        (Item::ArrayOfTables(old), Item::ArrayOfTables(new)) => {
            for (was, is) in old.iter().zip(new.iter_mut()) {
                *is.decor_mut() = was.decor().clone();
                is.set_position(was.position());
            }
        }
        _ => {}
    }
}
// ---------------------------------------------------------------------------

// ---- from the September tool, tedit-0.25.15/src/main.rs:63-80, rustfmt-formatted
fn mutate(v: &toml::Value, depth: usize, seed: &mut u64) -> toml::Value {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let r = (*seed >> 33) % 4;
    match v {
        toml::Value::String(s) => {
            if r == 0 {
                toml::Value::String(format!("{s}é\"\\\n\t😀'x"))
            } else {
                v.clone()
            }
        }
        toml::Value::Integer(i) => {
            if r == 0 {
                toml::Value::Integer(i.wrapping_add(1))
            } else {
                v.clone()
            }
        }
        toml::Value::Float(f) => {
            if r == 0 {
                toml::Value::Float(f * 1.5 + 0.1)
            } else {
                v.clone()
            }
        }
        toml::Value::Boolean(b) => {
            if r == 0 {
                toml::Value::Boolean(!b)
            } else {
                v.clone()
            }
        }
        toml::Value::Array(a) => {
            let mut a: Vec<_> = a.iter().map(|x| mutate(x, depth + 1, seed)).collect();
            if r == 1 {
                a.push(toml::Value::String("new\u{7f}elem".into()));
            }
            toml::Value::Array(a)
        }
        toml::Value::Table(t) => {
            let mut n = toml::Table::new();
            for (k, x) in t.iter() {
                if r == 2 && n.is_empty() {
                    continue;
                }
                n.insert(k.clone(), mutate(x, depth + 1, seed));
            }
            if r == 3 {
                n.insert(
                    "zz new key".into(),
                    toml::Value::String("inséré \"q\"".into()),
                );
                let mut sub = toml::Table::new();
                sub.insert("enabled".into(), toml::Value::Boolean(true));
                sub.insert(
                    "secret_encrypted".into(),
                    toml::Value::String("aes256:AAAA+/==".into()),
                );
                n.insert(format!("zz_tbl{depth}"), toml::Value::Table(sub));
            }
            toml::Value::Table(n)
        }
        toml::Value::Datetime(_) => v.clone(),
    }
}
// ---------------------------------------------------------------------------

/// The six mutation seeds of the September tool (`tedit-0.25.15/src/main.rs:105-106`).
const SEEDS: u64 = 6;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    /// `toml_edit` parsed it and `toml` 0.9.12 did too: every facet recorded.
    Ok,
    /// `toml_edit` parsed it but `toml` 0.9.12 refused: the three document
    /// facets recorded, no mutation scenario (the September tool's "(toml 0.9
    /// refuses)" arm).
    EditOnly,
    /// `toml_edit` refused it.
    ParseError,
    Unreadable,
}

impl Status {
    fn word(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::EditOnly => "edit-only",
            Status::ParseError => "parse-error",
            Status::Unreadable => "unreadable",
        }
    }

    fn from_word(w: &str) -> Option<Self> {
        Some(match w {
            "ok" => Status::Ok,
            "edit-only" => Status::EditOnly,
            "parse-error" => Status::ParseError,
            "unreadable" => Status::Unreadable,
            _ => return None,
        })
    }
}

struct Record {
    status: Status,
    /// SHA-256 over every recorded facet, length-prefixed; empty when nothing
    /// was recorded.
    fingerprint: String,
    /// Scenarios run (0 or 6) and, of those, how many re-parsed to a value
    /// other than the mutated one — the A08 round-trip defect.
    scenarios: u32,
    defects: u32,
}

struct Fingerprint(Sha256);

impl Fingerprint {
    fn new() -> Self {
        Fingerprint(Sha256::new())
    }

    fn feed(&mut self, label: &str, bytes: &[u8]) {
        self.0.update(label.as_bytes());
        self.0.update((bytes.len() as u64).to_le_bytes());
        self.0.update(bytes);
    }

    fn hex(self) -> String {
        self.0
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

fn record(path: &str) -> Record {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Record {
            status: Status::Unreadable,
            fingerprint: String::new(),
            scenarios: 0,
            defects: 0,
        };
    };
    let doc: DocumentMut = match text.parse() {
        Ok(d) => d,
        Err(_) => {
            return Record {
                status: Status::ParseError,
                fingerprint: String::new(),
                scenarios: 0,
                defects: 0,
            };
        }
    };

    let mut fp = Fingerprint::new();
    fp.feed("roundtrip-identical", &[u8::from(doc.to_string() == text)]);
    let mut fresh = DocumentMut::new();
    for (k, x) in doc.as_table().iter() {
        fresh.insert(k, fresh_item(x));
    }
    fp.feed("fresh", fresh.to_string().as_bytes());
    let mut formatted = doc.clone();
    formatted.fmt();
    fp.feed("fmt", formatted.to_string().as_bytes());

    // The mutation scenarios, with toml 0.9.12 as the typed side.
    let Ok(before) = toml::from_str::<toml::Table>(&text) else {
        return Record {
            status: Status::EditOnly,
            fingerprint: fp.hex(),
            scenarios: 0,
            defects: 0,
        };
    };
    let mut defects = 0;
    for s in 0..SEEDS {
        let mut seed = s * 7919 + 17;
        let toml::Value::Table(after) = mutate(&toml::Value::Table(before.clone()), 0, &mut seed)
        else {
            unreachable!("a table mutates to a table")
        };
        let label = format!("delta{s}");
        let Ok(rendered_text) = toml::to_string_pretty(&toml::Value::Table(after.clone())) else {
            fp.feed(&label, b"render-error");
            continue;
        };
        let Ok(rendered) = rendered_text.parse::<DocumentMut>() else {
            fp.feed(&label, b"rendered-parse-error");
            continue;
        };
        let mut target = doc.clone();
        apply_delta(target.as_table_mut(), &before, &after, rendered.as_table());
        let result = target.to_string();
        let reparse_equal = toml::from_str::<toml::Table>(&result).ok().as_ref() == Some(&after);
        fp.feed(&label, result.as_bytes());
        fp.feed(
            &format!("{label}-reparse-equal"),
            &[u8::from(reparse_equal)],
        );
        if !reparse_equal {
            defects += 1;
        }
    }
    Record {
        status: Status::Ok,
        fingerprint: fp.hex(),
        scenarios: SEEDS as u32,
        defects,
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: tedit record --out <file> <input>...\n       tedit compare --from <file> [--summary] [--roundtrip] <input>..."
    );
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("record") => cmd_record(&args[1..]),
        Some("compare") => cmd_compare(&args[1..]),
        _ => usage(),
    }
}

fn cmd_record(args: &[String]) {
    if args.len() < 3 || args[0] != "--out" {
        usage();
    }
    let out = &args[1];
    let inputs = &args[2..];
    let mut lines = String::from("# tedit record v1\n");
    for (i, path) in inputs.iter().enumerate() {
        let r = record(path);
        lines.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            i + 1,
            r.status.word(),
            r.fingerprint,
            r.scenarios,
            r.defects
        ));
    }
    if let Err(e) = std::fs::write(out, lines) {
        eprintln!("tedit: cannot write the record file: {e}");
        std::process::exit(2);
    }
}

fn read_record(path: &str, expected: usize) -> Vec<Record> {
    let Ok(text) = std::fs::read_to_string(path) else {
        eprintln!("tedit: cannot read the record file");
        std::process::exit(2);
    };
    let mut lines = text.lines();
    if lines.next() != Some("# tedit record v1") {
        eprintln!("tedit: not a tedit record file");
        std::process::exit(2);
    }
    let mut records = Vec::new();
    for (n, line) in lines.enumerate() {
        let f: Vec<&str> = line.split('\t').collect();
        let parsed = (f.len() == 5 && f[0] == (n + 1).to_string()).then(|| {
            Some(Record {
                status: Status::from_word(f[1])?,
                fingerprint: f[2].to_string(),
                scenarios: f[3].parse().ok()?,
                defects: f[4].parse().ok()?,
            })
        });
        match parsed.flatten() {
            Some(r) => records.push(r),
            None => {
                eprintln!("tedit: malformed record line {}", n + 2);
                std::process::exit(2);
            }
        }
    }
    if records.len() != expected {
        eprintln!(
            "tedit: the record holds {} inputs, the command line {}; pass the same inputs in the same order",
            records.len(),
            expected
        );
        std::process::exit(2);
    }
    records
}

#[derive(Default)]
struct Counts {
    inputs: usize,
    equal: usize,
    different: usize,
    parse_error_old: usize,
    parse_error_new: usize,
    parse_error_both: usize,
    unreadable: usize,
    typed_refused_old: usize,
    typed_refused_new: usize,
    scenarios_old: u32,
    scenarios_new: u32,
    defect_seeds_old: u32,
    defect_seeds_new: u32,
    inputs_with_defect_old: usize,
    inputs_with_defect_new: usize,
    inputs_with_defect_shared: usize,
}

fn cmd_compare(args: &[String]) {
    if args.len() < 2 || args[0] != "--from" {
        usage();
    }
    let from = &args[1];
    let mut rest = &args[2..];
    let (mut summary, mut roundtrip) = (false, false);
    while let Some(first) = rest.first() {
        match first.as_str() {
            "--summary" => summary = true,
            "--roundtrip" => roundtrip = true,
            _ => break,
        }
        rest = &rest[1..];
    }
    if rest.is_empty() {
        usage();
    }
    let olds = read_record(from, rest.len());

    let mut c = Counts::default();
    for (i, (path, old)) in rest.iter().zip(olds.iter()).enumerate() {
        let index = i + 1;
        let new = record(path);
        c.inputs += 1;
        c.scenarios_old += old.scenarios;
        c.scenarios_new += new.scenarios;
        c.defect_seeds_old += old.defects;
        c.defect_seeds_new += new.defects;
        if old.status == Status::EditOnly {
            c.typed_refused_old += 1;
        }
        if new.status == Status::EditOnly {
            c.typed_refused_new += 1;
        }
        let (old_defect, new_defect) = (old.defects > 0, new.defects > 0);
        if old_defect {
            c.inputs_with_defect_old += 1;
        }
        if new_defect {
            c.inputs_with_defect_new += 1;
        }
        if old_defect && new_defect {
            c.inputs_with_defect_shared += 1;
        }

        let old_parsed = matches!(old.status, Status::Ok | Status::EditOnly);
        let new_parsed = matches!(new.status, Status::Ok | Status::EditOnly);
        let word = match (old.status, new.status) {
            (Status::Unreadable, _) | (_, Status::Unreadable) => {
                c.unreadable += 1;
                "unreadable"
            }
            _ if !old_parsed && !new_parsed => {
                c.parse_error_both += 1;
                "parse-error-both"
            }
            _ if !old_parsed => {
                c.parse_error_old += 1;
                "parse-error-old"
            }
            _ if !new_parsed => {
                c.parse_error_new += 1;
                "parse-error-new"
            }
            _ if old.status == new.status && old.fingerprint == new.fingerprint => {
                c.equal += 1;
                "equal"
            }
            _ => {
                c.different += 1;
                "different"
            }
        };
        let roundtrip_word = match (old.scenarios > 0, new.scenarios > 0) {
            (false, _) | (_, false) => "not-applicable",
            _ => match (old_defect, new_defect) {
                (false, false) => "roundtrip-ok",
                (true, true) => "defect-shared",
                (true, false) => "defect-old-only",
                (false, true) => "defect-new-only",
            },
        };
        if !summary {
            println!("{index}\t{}", if roundtrip { roundtrip_word } else { word });
        }
    }

    if summary {
        println!("inputs\t{}", c.inputs);
        println!("equal\t{}", c.equal);
        println!("different\t{}", c.different);
        println!("parse-error-old\t{}", c.parse_error_old);
        println!("parse-error-new\t{}", c.parse_error_new);
        println!("parse-error-both\t{}", c.parse_error_both);
        println!("unreadable\t{}", c.unreadable);
        println!("typed-side-refused-old\t{}", c.typed_refused_old);
        println!("typed-side-refused-new\t{}", c.typed_refused_new);
        println!("scenarios-old\t{}", c.scenarios_old);
        println!("scenarios-new\t{}", c.scenarios_new);
        println!("roundtrip-defect-seeds-old\t{}", c.defect_seeds_old);
        println!("roundtrip-defect-seeds-new\t{}", c.defect_seeds_new);
        println!("roundtrip-defect-inputs-old\t{}", c.inputs_with_defect_old);
        println!("roundtrip-defect-inputs-new\t{}", c.inputs_with_defect_new);
        println!(
            "roundtrip-defect-inputs-shared\t{}",
            c.inputs_with_defect_shared
        );
    }
}
