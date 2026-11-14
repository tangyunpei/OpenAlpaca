//! The earlier builds' shipped files the tests use (subagent design §9):
//! each under `fixtures/<commit>/<path under config/>`, exactly as
//! committed, since a unit test cannot run `git show`.

/// `9ed847fe`'s `system_agent.md`: what a hand copy made before L9 holds,
/// with the two `require_confirmation_for: ["file_delete"]` lines.
pub(crate) const SYSTEM_AGENT_9ED847FE: &str =
    include_str!("fixtures/9ed847fe/agents/system_agent.md");
/// `eb98b820`'s `system_agent.md`: the last seeded version before `b79adc26`.
pub(crate) const SYSTEM_AGENT_EB98B820: &str =
    include_str!("fixtures/eb98b820/agents/system_agent.md");
/// `eb98b820`'s `writing_agent.md`: the last seeded version before `cea38040`.
pub(crate) const WRITING_AGENT_EB98B820: &str =
    include_str!("fixtures/eb98b820/agents/writing_agent.md");
/// `2bc29a80`'s `system_agent.md`: what `82022776`'s seeding wrote.
pub(crate) const SYSTEM_AGENT_2BC29A80: &str =
    include_str!("fixtures/2bc29a80/agents/system_agent.md");
/// `2bc29a80`'s `writing_agent.md`: what `82022776`'s seeding wrote.
pub(crate) const WRITING_AGENT_2BC29A80: &str =
    include_str!("fixtures/2bc29a80/agents/writing_agent.md");
/// `9b1ea1d3`'s `lead_agent.md`: the side before TEMPLATE-TIMEOUT (V-2b).
pub(crate) const LEAD_AGENT_9B1EA1D3: &str = include_str!("fixtures/9b1ea1d3/agents/lead_agent.md");
/// `e9f15916`'s `lead_agent.md`: the side TEMPLATE-TIMEOUT wrote (V-2b).
pub(crate) const LEAD_AGENT_E9F15916: &str = include_str!("fixtures/e9f15916/agents/lead_agent.md");
/// `4038c409`'s `lead_agent.md`: the side before TEMPLATE-TEMPERATURE, its
/// `temperature: 0.3` line still in it.
pub(crate) const LEAD_AGENT_4038C409: &str = include_str!("fixtures/4038c409/agents/lead_agent.md");

/// The Installation Manual's step 7, as written: delete exactly the two
/// lines `require_confirmation_for:` and `  - "file_delete"`.
pub(crate) fn step_7(text: &str) -> String {
    const LINES: &str = "require_confirmation_for:\n  - \"file_delete\"\n";
    assert_eq!(text.matches(LINES).count(), 1, "step 7 applies once");
    text.replace(LINES, "")
}

/// The Installation Manual's step 8, as written: add `  - "file_read"`
/// directly under `capabilities:`.
pub(crate) fn step_8(text: &str) -> String {
    assert_eq!(text.matches("\ncapabilities:\n").count(), 1);
    assert!(
        !text.contains("\"file_read\""),
        "step 8 applies to a copy without it"
    );
    text.replace("\ncapabilities:\n", "\ncapabilities:\n  - \"file_read\"\n")
}
