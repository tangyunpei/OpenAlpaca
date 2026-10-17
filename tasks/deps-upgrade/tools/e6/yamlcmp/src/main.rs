//! yamlcmp — the E6 comparator for SKILL.md frontmatter: serde_yaml 0.9.34
//! (what the tree reads with today) against serde-saphyr 1.3.0 with the options
//! P30 gives it, both in this one binary.
//!
//! `yamlcmp [--facet typed|value] <file>...` prints one line per input,
//! `<index>\t<word>`, and exits 0; `--summary` prints counts only. Nothing else
//! is printed: no value, no key, no path. The word is `equal`, `different`,
//! `parse-error-old`, `parse-error-new`, `parse-error-both`, `panic-old`,
//! `panic-new`, `panic-both`, `no-frontmatter` or `unreadable`.
//!
//! The word is decided on the **typed** facet by default — the frontmatter
//! read into the tree's own `SkillFrontmatter` and validated, which is what
//! `parse_skill_frontmatter` does (`crates/openalpaca_core/src/middleware/skill/mod.rs:51-56`).
//! `--facet value` decides it on the generic `serde_json::Value` read instead,
//! for a file whose real loader is not YAML at all (an agent template, a persona
//! document — their frontmatter only looks like YAML, and the typed facet says
//! nothing about them). Both facets are counted in `--summary`. A panic inside
//! either library is caught and is that side's `panic-*` word, counted as a
//! panic only (review F-13: never folded into a parse error); the panic hook
//! prints one fixed line on stderr and never the panic's own message, which
//! could quote the input. `E6_SELFTEST_PANIC=<index>[:old|new]` is the
//! self-test knob: reading that input, the named side (both when none is named)
//! panics on both facets with the input's path and text as its payload (README,
//! *Proving the panic path*).
//!
//! Inputs are the files named on the command line. Nothing is walked and
//! nothing is written: the September tool's serializer branch, which wrote
//! `render-*.yaml` beside its corpus for every `shipped/skill-` path, is gone.

#[allow(dead_code)]
mod types;

use std::panic::{AssertUnwindSafe, catch_unwind};
use types::SkillFrontmatter;

/// The options P30 gives serde-saphyr so it reads a file the way serde_yaml
/// 0.9 did (`yaml_options()` in the September `config.diff:326-335`): a YAML
/// 1.1 spelling like `yes` stays a string, a merge key is an ordinary key, the
/// last duplicate key wins, `.inf`/`.nan` are accepted, `!!binary` is ignored
/// for a string target, and an error carries no excerpt of the file.
fn yaml_options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        strict_booleans: true,
        merge_keys: serde_saphyr::MergeKeyPolicy::AsOrdinary,
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::LastWins,
        reject_non_finite_typeless_float: false,
        ignore_binary_tag_for_string: true,
        with_snippet: false,
    }
}

/// The YAML between the `---` delimiters — copied from
/// `crates/openalpaca_core/src/middleware/skill/parser.rs:5-29`, body dropped.
fn extract_frontmatter_str(input: &str) -> Option<&str> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_first = &trimmed[3..];
    let after_first = after_first.strip_prefix('\n').unwrap_or(after_first);
    let pos = after_first.find("\n---")?;
    Some(&after_first[..pos])
}

/// One library's reading of one facet. `Panic` is kept apart in the counts.
enum Outcome<T> {
    Ok(T),
    Err,
    Panic,
}

fn guarded<T>(f: impl FnOnce() -> Option<T>) -> Outcome<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Some(v)) => Outcome::Ok(v),
        Ok(None) => Outcome::Err,
        Err(_) => Outcome::Panic,
    }
}

/// Reading the input that `E6_SELFTEST_PANIC` names on this side: panic with
/// the input as the payload, as a library quoting its input would.
fn knob_panic(on_purpose: bool, payload: &str) {
    if on_purpose {
        panic!("{payload}");
    }
}

fn typed_old(yaml: &str, knob: (bool, &str)) -> Outcome<SkillFrontmatter> {
    guarded(|| {
        knob_panic(knob.0, knob.1);
        let fm: SkillFrontmatter = serde_yaml::from_str(yaml).ok()?;
        fm.validate().ok()?;
        Some(fm)
    })
}

fn typed_new(yaml: &str, knob: (bool, &str)) -> Outcome<SkillFrontmatter> {
    guarded(|| {
        knob_panic(knob.0, knob.1);
        let fm: SkillFrontmatter =
            serde_saphyr::from_str_with_options(yaml, yaml_options()).ok()?;
        fm.validate().ok()?;
        Some(fm)
    })
}

fn value_old(yaml: &str, knob: (bool, &str)) -> Outcome<serde_json::Value> {
    guarded(|| {
        knob_panic(knob.0, knob.1);
        serde_yaml::from_str::<serde_json::Value>(yaml).ok()
    })
}

fn value_new(yaml: &str, knob: (bool, &str)) -> Outcome<serde_json::Value> {
    guarded(|| {
        knob_panic(knob.0, knob.1);
        serde_saphyr::from_str_with_options::<serde_json::Value>(yaml, yaml_options()).ok()
    })
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
    eprintln!("yamlcmp: E6_SELFTEST_PANIC must be <index>[:old|new]");
    std::process::exit(2);
}

#[derive(Default)]
struct FacetCounts {
    equal: usize,
    different: usize,
    parse_error_old: usize,
    parse_error_new: usize,
    parse_error_both: usize,
    panic_old: usize,
    panic_new: usize,
    panic_both: usize,
}

impl FacetCounts {
    /// The word for one input on this facet, counting it once: a panic is
    /// reported before a parse error and counted as a panic only.
    fn word<T: PartialEq>(&mut self, old: Outcome<T>, new: Outcome<T>) -> &'static str {
        match (old, new) {
            (Outcome::Panic, Outcome::Panic) => {
                self.panic_both += 1;
                "panic-both"
            }
            (Outcome::Panic, _) => {
                self.panic_old += 1;
                "panic-old"
            }
            (_, Outcome::Panic) => {
                self.panic_new += 1;
                "panic-new"
            }
            (Outcome::Ok(a), Outcome::Ok(b)) => {
                if a == b {
                    self.equal += 1;
                    "equal"
                } else {
                    self.different += 1;
                    "different"
                }
            }
            (Outcome::Ok(_), _) => {
                self.parse_error_new += 1;
                "parse-error-new"
            }
            (_, Outcome::Ok(_)) => {
                self.parse_error_old += 1;
                "parse-error-old"
            }
            (_, _) => {
                self.parse_error_both += 1;
                "parse-error-both"
            }
        }
    }

    fn print(&self, facet: &str) {
        println!("{facet}:equal\t{}", self.equal);
        println!("{facet}:different\t{}", self.different);
        println!("{facet}:parse-error-old\t{}", self.parse_error_old);
        println!("{facet}:parse-error-new\t{}", self.parse_error_new);
        println!("{facet}:parse-error-both\t{}", self.parse_error_both);
        println!("{facet}:panic-old\t{}", self.panic_old);
        println!("{facet}:panic-new\t{}", self.panic_new);
        println!("{facet}:panic-both\t{}", self.panic_both);
    }
}

fn run() {
    let knob = selftest_panic();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut summary = false;
    let mut facet_value = false;
    while let Some(first) = args.first() {
        match first.as_str() {
            "--summary" => {
                summary = true;
                args.remove(0);
            }
            "--facet" => {
                args.remove(0);
                match args.first().map(String::as_str) {
                    Some("typed") => facet_value = false,
                    Some("value") => facet_value = true,
                    _ => {
                        eprintln!("usage: yamlcmp [--summary] [--facet typed|value] <file>...");
                        std::process::exit(2);
                    }
                }
                args.remove(0);
            }
            _ => break,
        }
    }
    if args.is_empty() {
        eprintln!("usage: yamlcmp [--summary] [--facet typed|value] <file>...");
        std::process::exit(2);
    }

    let (mut inputs, mut unreadable, mut no_frontmatter) = (0usize, 0usize, 0usize);
    let mut typed = FacetCounts::default();
    let mut value = FacetCounts::default();
    for (i, path) in args.iter().enumerate() {
        let index = i + 1;
        inputs += 1;
        let Ok(text) = std::fs::read_to_string(path) else {
            unreadable += 1;
            if !summary {
                println!("{index}\tunreadable");
            }
            continue;
        };
        let Some(yaml) = extract_frontmatter_str(&text) else {
            no_frontmatter += 1;
            if !summary {
                println!("{index}\tno-frontmatter");
            }
            continue;
        };
        let (knob_old, knob_new) = match knob {
            Some((at, old, new)) if at == index => (old, new),
            _ => (false, false),
        };
        let payload = format!("{path}\n{text}");
        let (ko, kn) = ((knob_old, payload.as_str()), (knob_new, payload.as_str()));
        let typed_word = typed.word(typed_old(yaml, ko), typed_new(yaml, kn));
        let value_word = value.word(value_old(yaml, ko), value_new(yaml, kn));
        if !summary {
            println!(
                "{index}\t{}",
                if facet_value { value_word } else { typed_word }
            );
        }
    }

    if summary {
        println!("inputs\t{inputs}");
        println!("unreadable\t{unreadable}");
        println!("no-frontmatter\t{no_frontmatter}");
        typed.print("typed");
        value.print("value");
    }
}

fn main() {
    // A panic inside a library is an outcome, not a message: its text could
    // quote the input, so the hook says only that one happened.
    std::panic::set_hook(Box::new(|_| {
        eprintln!("yamlcmp: a panic; its message is withheld because it could quote the input");
    }));
    // A deep document must end in a library's own error, never in a stack
    // overflow that loses the run.
    std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(run)
        .expect("spawn the worker thread")
        .join()
        .expect("the worker thread finished");
}
