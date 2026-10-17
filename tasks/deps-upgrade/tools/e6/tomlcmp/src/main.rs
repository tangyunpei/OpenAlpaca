//! tomlcmp — the E6 comparator for the `toml` crate: 0.9.12 (what the tree
//! holds today) against 1.1.6 (what P29 moves to), both in this one binary.
//!
//! `tomlcmp <file>...` prints one line per input, `<index>\t<word>`, and exits
//! 0. The index is the input's 1-based position on the command line; the word
//! is `equal`, `different`, `parse-error-old`, `parse-error-new`,
//! `parse-error-both`, `panic-old`, `panic-new`, `panic-both` or `unreadable`.
//! `tomlcmp --summary <file>...` prints counts only. Nothing else is ever
//! printed: no value, no key, no path.
//!
//! A panic inside either crate is caught per input and reported as that side's
//! `panic-*` word (a panic while one version reads what the other wrote is the
//! reading version's); the panic hook prints one fixed line on stderr and never
//! the panic's own message, which could quote the input (review F-13). A panic
//! row is counted as a panic only. `E6_SELFTEST_PANIC=<index>[:old|new]` is the
//! self-test knob: reading that input, the named side (both when none is named)
//! panics with the input's path and text as its payload (README, *Proving the
//! panic path*).
//!
//! `equal` means both versions parse the file and agree on every facet the
//! tree relies on: the value tree — datetimes compared by their components,
//! because a seconds-less time is `second: None` under toml 1.1 and `0` under
//! 0.9 while the TOML grammar means the same instant — the bytes of
//! `to_string` and of `to_string_pretty` (`render_config`'s path,
//! `crates/openalpaca_llm/src/config/llm_config/mod.rs:73`), and the `Display`
//! of every leaf (`config_codec::toml_to_json`'s datetime arm,
//! `crates/openalpaca_plugins/src/config_codec.rs:23`). Which facet differed,
//! and whether each version reads what the other writes, are counts in
//! `--summary`.
//!
//! Inputs are the files named on the command line; nothing is walked and
//! nothing is written.

use std::panic::{AssertUnwindSafe, catch_unwind};

/// The facets of one parsed document, as strings this program composes itself,
/// so that neither crate's own `Debug` of a tree decides equality.
struct Facets {
    tree: String,
    to_string: Option<String>,
    pretty: Option<String>,
    display: String,
}

macro_rules! for_crate {
    ($module:ident, $krate:ident) => {
        mod $module {
            use super::Facets;
            use std::fmt::Write as _;

            pub fn parse(text: &str) -> Option<$krate::Value> {
                $krate::from_str::<$krate::Value>(text).ok()
            }

            pub fn facets(v: &$krate::Value) -> Facets {
                let mut tree = String::new();
                canon(v, &mut tree);
                let mut display = String::new();
                leaves(v, &mut display);
                Facets {
                    tree,
                    to_string: $krate::to_string(v).ok(),
                    pretty: $krate::to_string_pretty(v).ok(),
                    display,
                }
            }

            /// A canonical, version-independent rendering of the value tree.
            fn canon(v: &$krate::Value, out: &mut String) {
                match v {
                    $krate::Value::String(s) => write!(out, "s{}:{s};", s.len()).unwrap(),
                    $krate::Value::Integer(i) => write!(out, "i{i};").unwrap(),
                    $krate::Value::Float(f) => write!(out, "f{f:?};").unwrap(),
                    $krate::Value::Boolean(b) => write!(out, "b{b};").unwrap(),
                    $krate::Value::Datetime(d) => {
                        out.push_str("d(");
                        match &d.date {
                            Some(x) => write!(out, "{}-{}-{}", x.year, x.month, x.day).unwrap(),
                            None => out.push('-'),
                        }
                        out.push(' ');
                        match &d.time {
                            // `second` and `nanosecond` are `u8`/`u32` in
                            // toml_datetime 0.7 and `Option<u8>`/`Option<u32>`
                            // in 1.1; `Option::from` reads both layouts, and an
                            // omitted second is 0 by the TOML 1.1 grammar.
                            Some(t) => write!(
                                out,
                                "{}:{}:{}.{}",
                                t.hour,
                                t.minute,
                                Option::<u8>::from(t.second).unwrap_or(0),
                                Option::<u32>::from(t.nanosecond).unwrap_or(0)
                            )
                            .unwrap(),
                            None => out.push('-'),
                        }
                        out.push(' ');
                        match &d.offset {
                            Some($krate::value::Offset::Z) => out.push('Z'),
                            Some($krate::value::Offset::Custom { minutes }) => {
                                write!(out, "{minutes}").unwrap()
                            }
                            None => out.push('-'),
                        }
                        out.push_str(");");
                    }
                    $krate::Value::Array(a) => {
                        out.push('[');
                        for x in a {
                            canon(x, out);
                        }
                        out.push(']');
                    }
                    $krate::Value::Table(t) => {
                        out.push('{');
                        for (k, x) in sorted(t) {
                            write!(out, "k{}:{k}=", k.len()).unwrap();
                            canon(x, out);
                        }
                        out.push('}');
                    }
                }
            }

            /// Every leaf's own `Display`, in canonical key order.
            fn leaves(v: &$krate::Value, out: &mut String) {
                match v {
                    $krate::Value::Table(t) => {
                        for (_, x) in sorted(t) {
                            leaves(x, out);
                        }
                    }
                    $krate::Value::Array(a) => a.iter().for_each(|x| leaves(x, out)),
                    other => {
                        let s = other.to_string();
                        write!(out, "{}:{s};", s.len()).unwrap();
                    }
                }
            }

            fn sorted(t: &$krate::Table) -> Vec<(&String, &$krate::Value)> {
                let mut entries: Vec<(&String, &$krate::Value)> = t.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                entries
            }
        }
    };
}

for_crate!(old, toml09);
for_crate!(new, toml11);

#[derive(Default)]
struct Counts {
    inputs: usize,
    equal: usize,
    different: usize,
    parse_error_old: usize,
    parse_error_new: usize,
    parse_error_both: usize,
    panic_old: usize,
    panic_new: usize,
    panic_both: usize,
    unreadable: usize,
    tree_different: usize,
    to_string_different: usize,
    pretty_different: usize,
    display_different: usize,
    new_reads_old_pretty_failed: usize,
    old_reads_new_pretty_failed: usize,
}

/// One side's reading of one input.
enum Outcome<T> {
    Ok(T),
    Err,
    Panic,
}

/// Run `f`, turning a panic into an outcome. The hook installed in `main`
/// keeps the panic's message off both streams.
fn guarded<T>(f: impl FnOnce() -> Option<T>) -> Outcome<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Some(v)) => Outcome::Ok(v),
        Ok(None) => Outcome::Err,
        Err(_) => Outcome::Panic,
    }
}

/// The self-test knob, `E6_SELFTEST_PANIC=<index>[:old|new]`: which input, and
/// which sides (`(old, new)`), panic on purpose. A malformed value is refused.
fn selftest_panic() -> Option<(usize, bool, bool)> {
    let raw = std::env::var("E6_SELFTEST_PANIC").ok()?;
    let (index, sides) = match raw.split_once(':') {
        None => (raw.as_str(), (true, true)),
        Some((i, "old")) => (i, (true, false)),
        Some((i, "new")) => (i, (false, true)),
        Some(_) => knob_refused(),
    };
    let Ok(index) = index.parse() else {
        knob_refused()
    };
    Some((index, sides.0, sides.1))
}

fn knob_refused() -> ! {
    eprintln!("tomlcmp: E6_SELFTEST_PANIC must be <index>[:old|new]");
    std::process::exit(2);
}

fn main() {
    // A panic's message could quote the input: say only that one happened.
    std::panic::set_hook(Box::new(|_| {
        eprintln!("tomlcmp: a panic; its message is withheld because it could quote the input");
    }));
    let knob = selftest_panic();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let summary = args.first().is_some_and(|a| a == "--summary");
    if summary {
        args.remove(0);
    }
    if args.is_empty() {
        eprintln!("usage: tomlcmp [--summary] <file>...");
        std::process::exit(2);
    }

    let mut c = Counts::default();
    for (i, path) in args.iter().enumerate() {
        let index = i + 1;
        c.inputs += 1;
        let Ok(text) = std::fs::read_to_string(path) else {
            c.unreadable += 1;
            if !summary {
                println!("{index}\tunreadable");
            }
            continue;
        };
        let (knob_old, knob_new) = match knob {
            Some((at, old, new)) if at == index => (old, new),
            _ => (false, false),
        };
        let old_side = guarded(|| {
            if knob_old {
                panic!("{path}\n{text}");
            }
            old::parse(&text).map(|v| old::facets(&v))
        });
        let new_side = guarded(|| {
            if knob_new {
                panic!("{path}\n{text}");
            }
            new::parse(&text).map(|v| new::facets(&v))
        });
        let word = match (old_side, new_side) {
            (Outcome::Panic, Outcome::Panic) => {
                c.panic_both += 1;
                "panic-both"
            }
            (Outcome::Panic, _) => {
                c.panic_old += 1;
                "panic-old"
            }
            (_, Outcome::Panic) => {
                c.panic_new += 1;
                "panic-new"
            }
            (Outcome::Err, Outcome::Err) => {
                c.parse_error_both += 1;
                "parse-error-both"
            }
            (Outcome::Err, Outcome::Ok(_)) => {
                c.parse_error_old += 1;
                "parse-error-old"
            }
            (Outcome::Ok(_), Outcome::Err) => {
                c.parse_error_new += 1;
                "parse-error-new"
            }
            (Outcome::Ok(fa), Outcome::Ok(fb)) => compare(&mut c, &fa, &fb),
        };
        if !summary {
            println!("{index}\t{word}");
        }
    }

    if summary {
        println!("inputs\t{}", c.inputs);
        println!("equal\t{}", c.equal);
        println!("different\t{}", c.different);
        println!("parse-error-old\t{}", c.parse_error_old);
        println!("parse-error-new\t{}", c.parse_error_new);
        println!("parse-error-both\t{}", c.parse_error_both);
        println!("panic-old\t{}", c.panic_old);
        println!("panic-new\t{}", c.panic_new);
        println!("panic-both\t{}", c.panic_both);
        println!("unreadable\t{}", c.unreadable);
        println!("facet:tree-different\t{}", c.tree_different);
        println!("facet:to_string-different\t{}", c.to_string_different);
        println!("facet:to_string_pretty-different\t{}", c.pretty_different);
        println!("facet:leaf-display-different\t{}", c.display_different);
        println!(
            "cross-read:new-reads-old-pretty-failed\t{}",
            c.new_reads_old_pretty_failed
        );
        println!(
            "cross-read:old-reads-new-pretty-failed\t{}",
            c.old_reads_new_pretty_failed
        );
    }
}

/// The word for an input both versions read, counting it. Every count it
/// touches is decided first, so a panic in a cross-read counts as that panic
/// alone.
fn compare(c: &mut Counts, fa: &Facets, fb: &Facets) -> &'static str {
    // The rollback question: does each version read what the other writes, to
    // the tree it holds itself? `None` is "not asked" (no pretty text).
    let new_reads_old = fa
        .pretty
        .as_deref()
        .map(|p| guarded(|| new::parse(p).map(|v| new::facets(&v).tree)));
    let old_reads_new = fb
        .pretty
        .as_deref()
        .map(|p| guarded(|| old::parse(p).map(|v| old::facets(&v).tree)));
    let new_panicked = matches!(new_reads_old, Some(Outcome::Panic));
    let old_panicked = matches!(old_reads_new, Some(Outcome::Panic));
    match (old_panicked, new_panicked) {
        (true, true) => {
            c.panic_both += 1;
            return "panic-both";
        }
        (true, false) => {
            c.panic_old += 1;
            return "panic-old";
        }
        (false, true) => {
            c.panic_new += 1;
            return "panic-new";
        }
        (false, false) => {}
    }
    let read_back = |r: Option<Outcome<String>>, own: &str| match r {
        None => true,
        Some(Outcome::Ok(tree)) => tree == own,
        Some(_) => false,
    };
    if !read_back(new_reads_old, &fb.tree) {
        c.new_reads_old_pretty_failed += 1;
    }
    if !read_back(old_reads_new, &fa.tree) {
        c.old_reads_new_pretty_failed += 1;
    }
    let mut same = true;
    if fa.tree != fb.tree {
        c.tree_different += 1;
        same = false;
    }
    if fa.to_string != fb.to_string {
        c.to_string_different += 1;
        same = false;
    }
    if fa.pretty != fb.pretty {
        c.pretty_different += 1;
        same = false;
    }
    if fa.display != fb.display {
        c.display_different += 1;
        same = false;
    }
    if same {
        c.equal += 1;
        "equal"
    } else {
        c.different += 1;
        "different"
    }
}
