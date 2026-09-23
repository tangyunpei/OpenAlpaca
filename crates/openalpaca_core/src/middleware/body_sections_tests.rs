//! The markdown body sectioning SKILL.md and agent templates share (CORE-03).
//!
//! Each row runs through both public entry points — `parse_skill_markdown`
//! and `parse_agent_markdown` — and expects the same `(body, sections)`.
//! The frontmatter in front of the body is each document's own business.

use std::collections::HashMap;

/// `(body, sections)` from the skill parser, then from the agent parser.
fn both(body: &str) -> [(String, HashMap<String, String>); 2] {
    let skill = super::skill::parse_skill_markdown(&format!(
        "---\nname: \"S\"\ndescription: \"d\"\n---\n{body}"
    ))
    .expect("skill parses");
    let agent = crate::agent::template::parse_agent_markdown(&format!(
        "---\nid: \"a\"\nname: \"A\"\ndescription: \"d\"\n---\n{body}"
    ))
    .expect("agent parses");
    [(skill.body, skill.sections), (agent.body, agent.sections)]
}

fn expect(body: &str, sections: &[(&str, &str)]) -> [(String, HashMap<String, String>); 2] {
    let sections: HashMap<String, String> = sections
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    [
        (body.to_string(), sections.clone()),
        (body.to_string(), sections),
    ]
}

#[test]
fn a_later_non_empty_duplicate_heading_overwrites_and_an_empty_one_does_not() {
    let body = "## A\n\nfirst\n\n## A\n\nsecond\n\n## B\n\nkeep\n\n## B\n\n";
    assert_eq!(
        both(body),
        expect(
            "## A\n\nfirst\n\n## A\n\nsecond\n\n## B\n\nkeep\n\n## B",
            &[("A", "second"), ("B", "keep")],
        )
    );
}

#[test]
fn an_empty_trailing_section_is_not_inserted() {
    let body = "## Only\n\ncontent\n\n## Empty\n\n   \n";
    assert_eq!(
        both(body),
        expect("## Only\n\ncontent\n\n## Empty", &[("Only", "content")])
    );
}

#[test]
fn text_before_the_first_heading_is_body_but_no_section() {
    // Leading blank lines are dropped; indentation and interior blanks stay.
    let body = "\n\n  intro line\n\nmore\n## First\nbody\n";
    assert_eq!(
        both(body),
        expect("  intro line\n\nmore\n## First\nbody", &[("First", "body")])
    );
}

#[test]
fn a_heading_inside_a_code_fence_is_still_a_heading() {
    let body = "## Real\n\n```\n## Fake\ncode\n```\n";
    assert_eq!(
        both(body),
        expect(
            "## Real\n\n```\n## Fake\ncode\n```",
            &[("Real", "```"), ("Fake", "code\n```")],
        )
    );
}

#[test]
fn only_a_trimmed_line_starting_with_hash_hash_space_is_a_heading() {
    // An indented `##` heading counts and its title is trimmed; a bare `##`,
    // a `## ` with nothing after it, and `###` are section content.
    let body = "   ##   Indented  \nx\n##\n## \n### Sub\n    y\n";
    assert_eq!(
        both(body),
        expect(
            "   ##   Indented  \nx\n##\n## \n### Sub\n    y",
            &[("Indented", "x\n##\n## \n### Sub\n    y")],
        )
    );
}
