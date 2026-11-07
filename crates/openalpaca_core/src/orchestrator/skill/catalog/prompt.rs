//! `<available_skills>` — the skill catalog as a prompt block (N-SKILL-1,
//! audit Z3-02).
//!
//! One renderer serves both models that hold `invoke_skill`: the main loop
//! (`Orchestrator::build_skills_catalog_block`, carried as the static
//! prompt's `skills_catalog` section) and the lead agent (`run_lead_agent`,
//! carried as a raw system block, since the lead's `SubagentMinimal` prompt
//! renders raw blocks only). Both read the availability-filtered
//! [`SkillCatalog::catalog_summary`], so neither lists a skill `/slash` or
//! `invoke_skill` would refuse.
//!
//! Each line names a skill by its **id** — the name `invoke_skill` resolves —
//! with its `/slash` command and its description cut to
//! [`AVAILABLE_SKILLS_DESCRIPTION_CHARS`]. The block has a character budget
//! (`[orchestrator.prompt_budgets] skills_catalog_budget`, counted over the
//! skill lines): when the full lines do not fit, every line is written as a
//! name only, and the list says so. A skill is never dropped for space. Only
//! the main loop's block adds that the user can type a skill's `/slash`
//! command ([`SkillsAudience`]): the lead runs no turn a person types into.

use super::{SkillCatalog, SkillSummary};

/// The most characters of a description one line carries; a longer one is
/// cut and ends with `…` (N-SKILL-5; ZCode's figure). The stock skills'
/// longest, create-skill's, is 199 characters, so each is listed whole.
pub const AVAILABLE_SKILLS_DESCRIPTION_CHARS: usize = 250;

/// What the model is told the list is for.
const HEADER: &str = "Run one of these skills with the invoke_skill tool: pass its id (or its /slash command) as `skill`.";

/// What the main loop is told besides: its user types the messages it answers.
const USER_SLASH: &str = " The user can also type a skill's /slash command.";

/// Which model reads the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillsAudience {
    /// The main loop, whose turns a person types — and can type a skill's
    /// `/slash` command into.
    MainLoop,
    /// The lead agent, which runs a workflow no person types into.
    Lead,
}

/// The line that says why a list carries no descriptions.
const NAMES_ONLY: &str =
    "The descriptions are left out so that every skill fits; each is listed by its id.";

impl SkillCatalog {
    /// The `<available_skills>` block for this catalog's available skills,
    /// within `budget_chars`, worded for `audience` — empty when no skill is
    /// available.
    pub fn available_skills_block(&self, budget_chars: usize, audience: SkillsAudience) -> String {
        render_available_skills(&self.catalog_summary(), budget_chars, audience)
    }
}

/// Render `skills` as the `<available_skills>` block for `audience`: full
/// lines when they fit `budget_chars`, every line a name only when they do
/// not. Empty when `skills` is.
pub fn render_available_skills(
    skills: &[SkillSummary],
    budget_chars: usize,
    audience: SkillsAudience,
) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let full: Vec<String> = skills.iter().map(|skill| line(skill, true)).collect();
    let full_chars: usize = full.iter().map(|l| l.chars().count()).sum();

    let mut block = String::from("<available_skills>\n");
    block.push_str(HEADER);
    if audience == SkillsAudience::MainLoop {
        block.push_str(USER_SLASH);
    }
    block.push('\n');
    if full_chars <= budget_chars {
        full.iter().for_each(|l| block.push_str(l));
    } else {
        block.push_str(NAMES_ONLY);
        block.push('\n');
        skills
            .iter()
            .for_each(|skill| block.push_str(&line(skill, false)));
    }
    block.push_str("</available_skills>");
    block
}

/// `- <id> (/<slash>): <description>`, without the parts a skill lacks, and
/// without the description when `with_description` is false.
fn line(skill: &SkillSummary, with_description: bool) -> String {
    let mut line = format!("- {}", skill.id);
    if let Some(command) = &skill.command {
        line.push_str(&format!(" (/{command})"));
    }
    if with_description {
        let description = one_line(&skill.description);
        if !description.is_empty() {
            line.push_str(": ");
            line.push_str(&description);
        }
    }
    line.push('\n');
    line
}

/// The description on one line — a folded or literal YAML scalar carries
/// line breaks — cut to [`AVAILABLE_SKILLS_DESCRIPTION_CHARS`] characters,
/// the ellipsis included.
fn one_line(description: &str) -> String {
    let collapsed = description.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= AVAILABLE_SKILLS_DESCRIPTION_CHARS {
        return collapsed;
    }
    let kept: String = collapsed
        .chars()
        .take(AVAILABLE_SKILLS_DESCRIPTION_CHARS - 1)
        .collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(id: &str, command: Option<&str>, description: &str) -> SkillSummary {
        SkillSummary {
            id: id.to_string(),
            name: id.to_uppercase(),
            description: description.to_string(),
            command: command.map(str::to_string),
        }
    }

    #[test]
    fn an_empty_catalog_renders_nothing() {
        assert_eq!(
            render_available_skills(&[], 500, SkillsAudience::MainLoop),
            ""
        );
    }

    #[test]
    fn a_long_description_is_cut_on_a_character_with_an_ellipsis() {
        let long = "é".repeat(300);
        let block = render_available_skills(
            &[skill("accents", None, &long)],
            10_000,
            SkillsAudience::MainLoop,
        );
        let line = block
            .lines()
            .find(|l| l.starts_with("- accents"))
            .expect("listed");
        let description = line.strip_prefix("- accents: ").expect("described");
        assert_eq!(
            description.chars().count(),
            AVAILABLE_SKILLS_DESCRIPTION_CHARS
        );
        assert!(description.ends_with('…'), "{line}");
    }

    #[test]
    fn a_folded_description_is_written_on_one_line() {
        let block = render_available_skills(
            &[skill(
                "create-skill",
                Some("create-skill"),
                "Guide the user\n  through it.\n",
            )],
            500,
            SkillsAudience::MainLoop,
        );
        assert!(
            block.contains("- create-skill (/create-skill): Guide the user through it.\n"),
            "{block}"
        );
    }

    /// The budget decides between two whole lists, never which skill stays.
    #[test]
    fn the_budget_switches_every_line_to_a_name_only() {
        let skills = [
            skill("alpha", Some("a"), "first skill's description"),
            skill("beta", None, "second skill's description"),
        ];
        let full_chars = "- alpha (/a): first skill's description\n".chars().count()
            + "- beta: second skill's description\n".chars().count();

        let fits = render_available_skills(&skills, full_chars, SkillsAudience::MainLoop);
        assert!(
            fits.contains("- alpha (/a): first skill's description\n"),
            "{fits}"
        );
        assert!(!fits.contains(NAMES_ONLY), "{fits}");

        let over = render_available_skills(&skills, full_chars - 1, SkillsAudience::MainLoop);
        assert!(over.contains(NAMES_ONLY), "{over}");
        assert!(over.contains("- alpha (/a)\n- beta\n"), "{over}");
        assert!(!over.contains("skill's description"), "{over}");
    }

    /// The budget a config with no `skills_catalog_budget` carries.
    fn default_budget() -> usize {
        crate::daemon_config::DaemonConfig::default()
            .orchestrator
            .prompt_budgets
            .skills_catalog_budget
    }

    /// N-SKILL-5: a description is listed whole to 250 characters; past them
    /// it is cut to 250, the ellipsis included (N-SKILL-1's cut was 100).
    #[test]
    fn a_description_is_whole_to_250_characters_and_cut_past_them() {
        let block = render_available_skills(
            &[
                skill("two-hundred", None, &"a".repeat(200)),
                skill("two-fifty", None, &"b".repeat(250)),
                skill(
                    "three-hundred",
                    None,
                    &format!("{}{}", "c".repeat(249), "d".repeat(51)),
                ),
            ],
            default_budget(),
            SkillsAudience::Lead,
        );
        let described = |id: &str| -> String {
            block
                .lines()
                .find_map(|l| l.strip_prefix(format!("- {id}: ").as_str()))
                .unwrap_or_else(|| panic!("{id} is not described:\n{block}"))
                .to_string()
        };
        assert_eq!(described("two-hundred"), "a".repeat(200));
        assert_eq!(described("two-fifty"), "b".repeat(250));
        let cut = described("three-hundred");
        assert_eq!(cut, format!("{}…", "c".repeat(249)));
        assert_eq!(cut.chars().count(), 250);
    }

    /// N-SKILL-5: twenty skills whose lines fit the default budget are each
    /// listed with the whole of its description, none by its id alone; the
    /// parent's 500 characters listed every one by its id.
    #[test]
    fn twenty_skills_within_the_default_budget_are_each_described() {
        let skills: Vec<SkillSummary> = (1..=20)
            .map(|n| {
                let id = format!("skill-{n:02}");
                let description = format!(
                    "Does the {id} job for the person who asks for it. {}",
                    "Then it reports what it did. ".repeat(6)
                );
                skill(&id, Some(&id), description.trim_end())
            })
            .collect();

        let block = render_available_skills(&skills, default_budget(), SkillsAudience::MainLoop);
        assert!(!block.contains(NAMES_ONLY), "{block}");
        for skill in &skills {
            let line = format!("- {0} (/{0}): {1}\n", skill.id, skill.description);
            assert!(block.contains(&line), "{line}{block}");
        }
    }

    /// N-SKILL-5: the default budget holds 20,000 characters of skill lines —
    /// the key's clamp maximum — and one more lists every skill by its id
    /// alone: the fallback is kept.
    #[test]
    fn the_default_budget_holds_twenty_thousand_characters_of_lines() {
        // Eighty lines of 250 characters: `- s-01: `, a description of 241,
        // the line break.
        let mut skills: Vec<SkillSummary> = (1..=80)
            .map(|n| skill(&format!("s-{n:02}"), None, &"x".repeat(241)))
            .collect();
        let fits = render_available_skills(&skills, default_budget(), SkillsAudience::Lead);
        let line_chars: usize = fits
            .lines()
            .filter(|l| l.starts_with("- "))
            .map(|l| l.chars().count() + 1)
            .sum();
        assert_eq!(line_chars, 20_000, "{fits}");
        assert!(!fits.contains(NAMES_ONLY), "{fits}");

        skills[0].description.push('x');
        let over = render_available_skills(&skills, default_budget(), SkillsAudience::Lead);
        assert!(over.contains(NAMES_ONLY), "{over}");
        assert!(over.contains("- s-01\n- s-02\n"), "{over}");
        assert!(!over.contains("xxx"), "{over}");
    }

    /// N-SKILL-5: the four stock skills' block, as the main loop's is
    /// rendered, each skill with the whole of its description. The parent's
    /// differed in create-skill's line alone: that description, 199
    /// characters, was cut at 100 ("… Collects configurati…"); the other
    /// three are under 100, and the four lines, 497 characters, fit the
    /// budget.
    #[test]
    fn the_stock_skills_are_each_listed_with_the_whole_of_its_description() {
        let shipped = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/skills");
        let catalog = SkillCatalog::new();
        assert_eq!(
            catalog.scan_directory(&shipped, crate::middleware::skill::SkillScope::Project),
            4
        );

        let block = catalog.available_skills_block(default_budget(), SkillsAudience::MainLoop);
        let expected = format!(
            "<available_skills>\n{HEADER}{USER_SLASH}\n{}</available_skills>",
            concat!(
                "- code-review (/review): Review code for bugs, style issues, security concerns, \
                 and improvements\n",
                "- commit-message (/commit): Generate conventional commit messages from code \
                 changes\n",
                "- create-skill (/create-skill): Interactively guide the user through creating a \
                 new SKILL.md skill definition. Collects configuration, validates inputs, \
                 scaffolds the directory, and writes the skill file using bundled script \
                 tools.\n",
                "- explain-code (/explain-code): Explain what a piece of code does in plain \
                 language\n",
            )
        );
        let differing: Vec<(&str, &str)> = block
            .lines()
            .zip(expected.lines())
            .filter(|(rendered, pinned)| rendered != pinned)
            .collect();
        assert_eq!(
            block, expected,
            "the lines that differ, (rendered, pinned): {differing:#?}"
        );
    }

    /// The main loop's user can type a skill's slash command; the lead's
    /// block, otherwise the same, says nothing about a user.
    #[test]
    fn only_the_main_loop_is_told_the_user_types_slash_commands() {
        let skills = [skill("code-review", Some("review"), "Review code")];
        let main_loop = render_available_skills(&skills, 500, SkillsAudience::MainLoop);
        let lead = render_available_skills(&skills, 500, SkillsAudience::Lead);

        assert!(
            main_loop.contains(&format!("{HEADER}{USER_SLASH}\n")),
            "{main_loop}"
        );
        assert!(lead.contains(&format!("{HEADER}\n")), "{lead}");
        assert!(!lead.contains("user"), "{lead}");
        assert_eq!(main_loop.replacen(USER_SLASH, "", 1), lead);
    }
}
