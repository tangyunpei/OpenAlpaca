//! keycount — the DU-10 count (plan-rev3 §5.1 item 3, review SF-14): how many
//! keys named `secret_ref`, and how many `use_keychain` keys set to `true`, a
//! TOML file holds at any depth, read with the `toml` crate the daemon reads
//! these files with (0.9.12) — a `[[providers.<name>.keys]]` entry of
//! `llm.toml`, the inline table `key = { secret_ref = … }` the plugin manager
//! writes into `plugins/.config/<name>.toml`
//! (`crates/openalpaca_plugins/src/permission_gate.rs:446`), a dotted key, an
//! array of tables — so neither the file nor the spelling decides whether a
//! reference is seen. A key is counted whatever its value: the count errs high,
//! never low, because a 0 waives P12's round trip against the real keychain.
//!
//!   keycount [--summary] <file>...
//!
//! One line per input, `<index>\t<word>\t<secret_ref>\t<use_keychain-true>`;
//! the index is the input's 1-based position on the command line and the word
//! is `counted` (the two counts follow), or `parse-error`, `unreadable` or
//! `panic` (each followed by `-\t-`: nothing was counted). `--summary` prints
//! totals only. Exits 0. Nothing else is printed — no value, no key but the two
//! it counts by name, no path — and nothing is written; nothing is walked, the
//! inputs are the files named.
//!
//! A panic inside the parser is caught per input and printed as `panic`; the
//! panic hook prints one fixed line on stderr and never the panic's own
//! message, which could quote the input (review F-13). `E6_SELFTEST_PANIC=<index>`
//! is the self-test knob: that input panics on purpose, with its path and text
//! as the payload (a `:old`/`:new` suffix is accepted and ignored; README,
//! *Proving the panic path*).

use std::panic::{AssertUnwindSafe, catch_unwind};

/// The two counts of one document.
#[derive(Default)]
struct Found {
    secret_ref: u64,
    use_keychain_true: u64,
}

/// Count the two keys in `value` and everything below it.
fn count(value: &toml::Value, found: &mut Found) {
    match value {
        toml::Value::Table(table) => {
            for (key, item) in table {
                if key == "secret_ref" {
                    found.secret_ref += 1;
                }
                if key == "use_keychain" && item.as_bool() == Some(true) {
                    found.use_keychain_true += 1;
                }
                count(item, found);
            }
        }
        toml::Value::Array(items) => items.iter().for_each(|item| count(item, found)),
        _ => {}
    }
}

enum Outcome {
    Counted(Found),
    ParseError,
    Unreadable,
    Panic,
}

fn read_and_count(path: &str, panic_on_purpose: bool) -> Outcome {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Outcome::Unreadable;
    };
    let parsed = catch_unwind(AssertUnwindSafe(|| {
        if panic_on_purpose {
            panic!("{path}\n{text}");
        }
        let table = toml::from_str::<toml::Table>(&text).ok()?;
        let mut found = Found::default();
        count(&toml::Value::Table(table), &mut found);
        Some(found)
    }));
    match parsed {
        Ok(Some(found)) => Outcome::Counted(found),
        Ok(None) => Outcome::ParseError,
        Err(_) => Outcome::Panic,
    }
}

/// The self-test knob, `E6_SELFTEST_PANIC=<index>[:old|new]`: the input that
/// panics on purpose (the side suffix is accepted and ignored). A malformed
/// value is refused.
fn selftest_panic() -> Option<usize> {
    let raw = std::env::var("E6_SELFTEST_PANIC").ok()?;
    let index = match raw.split_once(':') {
        None => raw.as_str(),
        Some((i, "old" | "new")) => i,
        Some(_) => knob_refused(),
    };
    let Ok(index) = index.parse() else {
        knob_refused()
    };
    Some(index)
}

fn knob_refused() -> ! {
    eprintln!("keycount: E6_SELFTEST_PANIC must be <index>[:old|new]");
    std::process::exit(2);
}

#[derive(Default)]
struct Totals {
    inputs: usize,
    counted: usize,
    parse_error: usize,
    unreadable: usize,
    panic: usize,
    secret_ref: u64,
    use_keychain_true: u64,
    inputs_with_secret_ref: usize,
    inputs_with_use_keychain_true: usize,
}

fn main() {
    // A panic's message could quote the input: say only that one happened.
    std::panic::set_hook(Box::new(|_| {
        eprintln!("keycount: a panic; its message is withheld because it could quote the input");
    }));
    let knob = selftest_panic();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let summary = args.first().is_some_and(|a| a == "--summary");
    if summary {
        args.remove(0);
    }
    if args.is_empty() {
        eprintln!("usage: keycount [--summary] <file>...");
        std::process::exit(2);
    }

    let mut t = Totals::default();
    for (i, path) in args.iter().enumerate() {
        let index = i + 1;
        t.inputs += 1;
        let line = match read_and_count(path, knob == Some(index)) {
            Outcome::Counted(found) => {
                t.counted += 1;
                t.secret_ref += found.secret_ref;
                t.use_keychain_true += found.use_keychain_true;
                if found.secret_ref > 0 {
                    t.inputs_with_secret_ref += 1;
                }
                if found.use_keychain_true > 0 {
                    t.inputs_with_use_keychain_true += 1;
                }
                format!(
                    "{index}\tcounted\t{}\t{}",
                    found.secret_ref, found.use_keychain_true
                )
            }
            Outcome::ParseError => {
                t.parse_error += 1;
                format!("{index}\tparse-error\t-\t-")
            }
            Outcome::Unreadable => {
                t.unreadable += 1;
                format!("{index}\tunreadable\t-\t-")
            }
            Outcome::Panic => {
                t.panic += 1;
                format!("{index}\tpanic\t-\t-")
            }
        };
        if !summary {
            println!("{line}");
        }
    }

    if summary {
        println!("inputs\t{}", t.inputs);
        println!("counted\t{}", t.counted);
        println!("parse-error\t{}", t.parse_error);
        println!("unreadable\t{}", t.unreadable);
        println!("panic\t{}", t.panic);
        println!("secret_ref\t{}", t.secret_ref);
        println!("use_keychain-true\t{}", t.use_keychain_true);
        println!("inputs-with-secret_ref\t{}", t.inputs_with_secret_ref);
        println!(
            "inputs-with-use_keychain-true\t{}",
            t.inputs_with_use_keychain_true
        );
    }
}
