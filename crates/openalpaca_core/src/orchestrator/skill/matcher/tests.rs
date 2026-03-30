use super::*;
use crate::agent::subagent::{
    AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Capability, SubAgent,
};

fn make_agent(id: &str, name: &str, skills: Vec<&str>) -> SubAgent {
    SubAgent {
        id: id.to_string(),
        template_id: id.to_string(),
        name: name.to_string(),
        description: Some(format!("{} agent", name)),
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        capabilities: skills
            .into_iter()
            .map(|s| Capability {
                name: s.to_string(),
                category: "test".to_string(),
                proficiency: 1.0,
            })
            .collect(),
        preset: AgentPreset::default(),
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    }
}

fn make_registry(agents: Vec<SubAgent>) -> AgentRegistry {
    let reg = AgentRegistry::new();
    for a in agents {
        reg.register(a);
    }
    reg
}

#[test]
fn test_single_skill() {
    let reg = make_registry(vec![make_agent("a1", "Searcher", vec!["web_search"])]);
    let matcher = SkillMatcher;
    let result = matcher
        .match_skills(&["web_search".to_string()], &reg)
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].agent_id, "a1");
    assert_eq!(result[0].matched_skills, vec!["web_search"]);
}

#[test]
fn test_multi_skills_single_agent() {
    let reg = make_registry(vec![make_agent(
        "a1",
        "MultiTool",
        vec!["web_search", "summarize", "text_generate"],
    )]);
    let matcher = SkillMatcher;
    let result = matcher
        .match_skills(
            &[
                "web_search".to_string(),
                "summarize".to_string(),
                "text_generate".to_string(),
            ],
            &reg,
        )
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].matched_skills.len(), 3);
}

#[test]
fn test_multi_agents() {
    let reg = make_registry(vec![
        make_agent("a1", "Searcher", vec!["web_search"]),
        make_agent("a2", "Writer", vec!["text_generate"]),
    ]);
    let matcher = SkillMatcher;
    let result = matcher
        .match_skills(
            &["web_search".to_string(), "text_generate".to_string()],
            &reg,
        )
        .unwrap();
    assert_eq!(result.len(), 2);
}

#[test]
fn test_no_idle() {
    let reg = AgentRegistry::new();
    let agent = make_agent("a1", "Searcher", vec!["web_search"]);
    reg.register(agent);
    reg.update_status(
        "a1",
        AgentStatus::Busy {
            task_id: "t1".into(),
        },
    );

    let matcher = SkillMatcher;
    let result = matcher.match_skills(&["web_search".to_string()], &reg);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("No idle agents"));
}

#[test]
fn test_no_matching() {
    let reg = make_registry(vec![make_agent("a1", "Searcher", vec!["web_search"])]);
    let matcher = SkillMatcher;
    let result = matcher.match_skills(&["text_generate".to_string()], &reg);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("No agents match the required capabilities"));
}

#[test]
fn test_empty_skills() {
    let reg = make_registry(vec![make_agent("a1", "Searcher", vec!["web_search"])]);
    let matcher = SkillMatcher;
    let result = matcher.match_skills(&[], &reg);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("No capabilities specified"));
}

#[test]
fn test_greedy_prefers_more_skills() {
    // a1 has 1 skill, a2 has 2 of the required skills -> a2 should be picked first
    let reg = make_registry(vec![
        make_agent("a1", "Narrow", vec!["web_search"]),
        make_agent("a2", "Broad", vec!["web_search", "summarize"]),
    ]);
    let matcher = SkillMatcher;
    let result = matcher
        .match_skills(&["web_search".to_string(), "summarize".to_string()], &reg)
        .unwrap();
    // Greedy should pick a2 first (covers both), so only 1 agent needed
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].agent_id, "a2");
    assert_eq!(result[0].matched_skills.len(), 2);
}
