//! yamlcmp — the E6 comparator for SKILL.md frontmatter: serde_yaml 0.9.34
//! (what the tree reads with today) against serde-saphyr 1.3.0 with the options
//! P30 gives it, both in this one binary.
//!
//! `yamlcmp [--facet typed|value] <file>...` prints one line per input,
//! `<index>\t<word>`, and exits 0; `--summary` prints counts only. Nothing else
//! is printed: no value, no key, no path. The word is `equal`, `different`,
//! `parse-error-old`, `parse-error-new`, `parse-error-both`, `no-frontmatter`
//! or `unreadable`.
//!
//! The word is decided on the **typed** facet by default — the frontmatter
//! read into the tree's own `SkillFrontmatter` and validated, which is what
//! `parse_skill_frontmatter` does (`crates/openalpaca_core/src/middleware/skill/mod.rs:51-56`).
//! `--facet value` decides it on the generic `serde_json::Value` read instead,
//! for a file whose real loader is not YAML at all (an agent template, a persona
//! document — their frontmatter only looks like YAML, and the typed facet says
//! nothing about them). Both facets are counted in `--summary`; a panic inside
//! either library counts as that side's parse error.
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

fn typed_old(yaml: &str) -> Outcome<SkillFrontmatter> {
    guarded(|| {
        let fm: SkillFrontmatter = serde_yaml::from_str(yaml).ok()?;
        fm.validate().ok()?;
        Some(fm)
    })
}

fn typed_new(yaml: &str) -> Outcome<SkillFrontmatter> {
    guarded(|| {
        let fm: SkillFrontmatter =
            serde_saphyr::from_str_with_options(yaml, yaml_options()).ok()?;
        fm.validate().ok()?;
        Some(fm)
    })
}

fn value_old(yaml: &str) -> Outcome<serde_json::Value> {
    guarded(|| serde_yaml::from_str::<serde_json::Value>(yaml).ok())
}

fn value_new(yaml: &str) -> Outcome<serde_json::Value> {
    guarded(|| serde_saphyr::from_str_with_options::<serde_json::Value>(yaml, yaml_options()).ok())
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
}

impl FacetCounts {
    /// The word for one input on this facet, counting it.
    fn word<T: PartialEq>(&mut self, old: Outcome<T>, new: Outcome<T>) -> &'static str {
        if matches!(old, Outcome::Panic) {
            self.panic_old += 1;
        }
        if matches!(new, Outcome::Panic) {
            self.panic_new += 1;
        }
        match (old, new) {
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
    }
}

fn run() {
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
        let typed_word = typed.word(typed_old(yaml), typed_new(yaml));
        let value_word = value.word(value_old(yaml), value_new(yaml));
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
    // quote the input.
    std::panic::set_hook(Box::new(|_| {}));
    // A deep document must end in a library's own error, never in a stack
    // overflow that loses the run.
    std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(run)
        .expect("spawn the worker thread")
        .join()
        .expect("the worker thread finished");
}
