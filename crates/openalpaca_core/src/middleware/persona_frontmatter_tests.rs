//! One table over the four persona parsers' frontmatter rules (CORE-02).
//!
//! SOUL, USER, IDENTITY and BOOTSTRAP read the same hand-rolled frontmatter
//! grammar — not YAML — and differ only in which fields they require: SOUL
//! and USER require `title`, IDENTITY and BOOTSTRAP do not read it at all.
//! Every row runs through all four public entry points so the shared grammar
//! and the per-document differences are pinned in one place.

use super::{bootstrap, identity, soul, user};

/// `(title, summary, read_when)` on success, the error's `Debug` text on
/// failure. The four error enums spell their shared variants the same way.
type Outcome = Result<(Option<String>, String, Vec<String>), String>;

/// A body that satisfies SOUL's required sections and is harmless to the
/// other three, so a row fails or passes on its frontmatter alone.
const SOUL_BODY: &str = "\n## Core Truths\n\nBe useful.\n\n## Boundaries\n\n- Stay safe.\n\n## Vibe\n\nCalm.\n\n## Continuity\n\nRemember.\n";

/// SOUL, USER, IDENTITY, BOOTSTRAP — in that order.
fn parse_all(frontmatter: &str) -> [Outcome; 4] {
    let doc = format!("{frontmatter}{SOUL_BODY}");
    [
        soul::parse_soul_markdown(&doc)
            .map(|d| {
                let f = d.frontmatter;
                (Some(f.title), f.summary, f.read_when)
            })
            .map_err(|e| format!("{e:?}")),
        user::parse_user_markdown(&doc)
            .map(|d| {
                let f = d.frontmatter;
                (Some(f.title), f.summary, f.read_when)
            })
            .map_err(|e| format!("{e:?}")),
        identity::parse_identity_markdown(&doc)
            .map(|d| (None, d.frontmatter.summary, d.frontmatter.read_when))
            .map_err(|e| format!("{e:?}")),
        bootstrap::parse_bootstrap_markdown(&doc)
            .map(|d| (None, d.frontmatter.summary, d.frontmatter.read_when))
            .map_err(|e| format!("{e:?}")),
    ]
}

fn ok(title: &str, summary: &str, read_when: &[&str]) -> [Outcome; 4] {
    let read_when: Vec<String> = read_when.iter().map(|s| s.to_string()).collect();
    let titled = Ok((
        Some(title.to_string()),
        summary.to_string(),
        read_when.clone(),
    ));
    let untitled = Ok((None, summary.to_string(), read_when));
    [titled.clone(), titled, untitled.clone(), untitled]
}

fn err_all(debug: &str) -> [Outcome; 4] {
    std::array::from_fn(|_| Err(debug.to_string()))
}

#[test]
fn quotes_are_stripped_only_when_they_match_and_the_inside_is_trimmed() {
    let fm = "---\n\
              title: \"  Quoted Title  \"\n\
              summary: 'single'\n\
              read_when:\n  \
              - \"double item\"\n  \
              - 'single item'\n  \
              - \"mismatched'\n  \
              - \"\n  \
              - plain\n\
              ---\n";
    assert_eq!(
        parse_all(fm),
        ok(
            "Quoted Title",
            "single",
            &["double item", "single item", "\"mismatched'", "\"", "plain"],
        )
    );
}

#[test]
fn an_empty_value_counts_as_present() {
    // `summary: ""` and a bare `title:` are both present-and-empty, not missing.
    let fm = "---\ntitle:\nsummary: \"\"\nread_when:\n  - a\n---\n";
    assert_eq!(parse_all(fm), ok("", "", &["a"]));
}

#[test]
fn a_key_needs_no_space_after_its_colon() {
    let fm = "---\ntitle:T\nsummary:S\nread_when:\n  - a\n---\n";
    assert_eq!(parse_all(fm), ok("T", "S", &["a"]));
}

#[test]
fn unknown_keys_are_tolerated_and_a_non_item_line_ends_read_when() {
    // Blank lines inside the list are skipped; `note: x` ends it, and the
    // `- c` / `- d` items after it belong to no list and are dropped.
    let fm = "---\n\
              title: T\n\
              owner: someone\n\
              summary: S\n\
              read_when:\n  \
              - a\n\
              \n  \
              - b\n  \
              note: x\n  \
              - c\n\
              nested:\n  \
              - d\n\
              ---\n";
    assert_eq!(parse_all(fm), ok("T", "S", &["a", "b"]));
}

#[test]
fn a_duplicate_scalar_keeps_the_last_and_a_duplicate_list_appends() {
    let fm = "---\n\
              title: First\n\
              title: Second\n\
              summary: one\n\
              summary: two\n\
              read_when:\n  \
              - a\n\
              read_when:\n  \
              - b\n\
              ---\n";
    assert_eq!(parse_all(fm), ok("Second", "two", &["a", "b"]));
}

#[test]
fn an_empty_or_inline_read_when_is_missing() {
    for fm in [
        "---\ntitle: T\nsummary: S\nread_when:\n---\n",
        "---\ntitle: T\nsummary: S\nread_when: [a, b]\n---\n",
        "---\ntitle: T\nsummary: S\nread_when:\n  -a\n---\n",
    ] {
        assert_eq!(
            parse_all(fm),
            err_all("MissingField(\"read_when\")"),
            "input: {fm:?}"
        );
    }
}

#[test]
fn required_fields_fail_in_title_summary_read_when_order() {
    // Nothing at all: SOUL and USER miss `title` first; IDENTITY and
    // BOOTSTRAP never ask for it, so they miss `summary` first.
    let title = Err("MissingField(\"title\")".to_string());
    let summary = Err("MissingField(\"summary\")".to_string());
    assert_eq!(
        parse_all("---\n---\n"),
        [title.clone(), title, summary.clone(), summary.clone()]
    );
    // Title present: everyone misses `summary` next.
    assert_eq!(
        parse_all("---\ntitle: T\n---\n"),
        err_all("MissingField(\"summary\")")
    );
    // Summary present, no title: read_when is still checked after title.
    let title = Err("MissingField(\"title\")".to_string());
    let read_when = Err("MissingField(\"read_when\")".to_string());
    assert_eq!(
        parse_all("---\nsummary: S\n---\n"),
        [title.clone(), title, read_when.clone(), read_when]
    );
}

#[test]
fn the_opening_delimiter_must_be_the_first_line() {
    for fm in [
        "",
        "no frontmatter",
        "\n---\ntitle: T\nsummary: S\nread_when:\n  - a\n---\n",
    ] {
        assert_eq!(
            parse_all(fm),
            err_all("MissingFrontmatter"),
            "input: {fm:?}"
        );
    }
}

#[test]
fn a_missing_opening_delimiter_is_reported_before_a_missing_close() {
    assert_eq!(
        parse_all("title: T\nsummary: S\n"),
        err_all("MissingFrontmatter")
    );
    assert_eq!(
        parse_all("---\ntitle: T\nsummary: S\nread_when:\n  - a\n"),
        err_all("UnterminatedFrontmatter")
    );
}

#[test]
fn delimiters_are_trimmed_and_crlf_is_read() {
    let fm = "  ---  \r\ntitle: T\r\nsummary: S\r\nread_when:\r\n  - a\r\n --- \r\n";
    assert_eq!(parse_all(fm), ok("T", "S", &["a"]));
}

#[test]
fn soul_reports_a_frontmatter_error_before_a_body_error() {
    let err = soul::parse_soul_markdown("---\nsummary: S\n---\nno sections here\n")
        .expect_err("missing title and missing sections");
    assert_eq!(err, soul::SoulParseError::MissingField("title"));
}

#[test]
fn bootstrap_keeps_a_later_delimiter_in_its_body() {
    let doc = bootstrap::parse_bootstrap_markdown(
        "---\nsummary: S\nread_when:\n  - a\n---\n\nfirst\n---\nsecond\n\n",
    )
    .expect("parses");
    assert_eq!(doc.body, "first\n---\nsecond");
}
