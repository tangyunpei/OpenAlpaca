    use super::*;
    use crate::agent::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, Skill};
    use crate::agent::template::{AgentTemplate, AgentTemplateFrontmatter, parse_agent_markdown};

    fn make_agent(id: &str, skills: Vec<&str>) -> SubAgent {
        SubAgent {
            id: id.to_string(),
            template_id: id.to_string(),
            name: format!("Agent {}", id),
            description: None,
            icon: None,
            status: AgentStatus::Idle,
            current_task: None,
            skills: skills
                .into_iter()
                .map(|s| Skill {
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

    fn make_template(id: &str, skills: Vec<&str>, singleton: bool) -> AgentTemplate {
        AgentTemplate {
            frontmatter: AgentTemplateFrontmatter {
                id: id.to_string(),
                name: format!("Template {}", id),
                description: format!("{} template", id),
                icon: None,
                singleton,
                skills: skills.into_iter().map(|s| s.to_string()).collect(),
                denied_skills: vec![],
                temperature: 0.5,
                verbosity: "normal".to_string(),
                model: None,
                fallback_models: vec![],
                max_tool_calls: None,
                timeout_seconds: None,
                max_cost_per_task: None,
                max_rounds: None,
                require_confirmation_for: vec![],
            },
            body: String::new(),
            sections: std::collections::HashMap::new(),
        }
    }

    // ── Legacy (backward compat) tests ─────────────────────────────

    #[test]
    fn test_register_and_get() {
        let reg = AgentRegistry::new();
        assert!(reg.register(make_agent("a1", vec!["search"])));
        assert!(!reg.register(make_agent("a1", vec!["search"]))); // duplicate
        assert_eq!(reg.count(), 1);

        let agent = reg.get("a1").unwrap();
        assert_eq!(agent.name, "Agent a1");
    }

    #[test]
    fn test_get_nonexistent() {
        let reg = AgentRegistry::new();
        assert!(reg.get("nope").is_none());
    }

    #[test]
    fn test_remove() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec![]));
        assert!(reg.remove("a1"));
        assert!(!reg.remove("a1"));
        assert_eq!(reg.count(), 0);
    }

    #[test]
    fn test_update_status() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec![]));

        assert!(reg.update_status(
            "a1",
            AgentStatus::Busy {
                task_id: "t1".into()
            }
        ));
        let agent = reg.get("a1").unwrap();
        assert_eq!(agent.status.as_str(), "busy");
        assert_eq!(agent.current_task.as_deref(), Some("t1"));

        assert!(reg.update_status("a1", AgentStatus::Idle));
        let agent = reg.get("a1").unwrap();
        assert!(agent.status.is_available());
        assert!(agent.current_task.is_none());

        assert!(!reg.update_status("nope", AgentStatus::Idle));
    }

    #[test]
    fn test_list_instances() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec![]));
        reg.register(make_agent("a2", vec![]));

        let all = reg.list_instances();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_list_idle() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec![]));
        reg.register(make_agent("a2", vec![]));

        reg.update_status(
            "a1",
            AgentStatus::Busy {
                task_id: "t1".into(),
            },
        );

        let idle = reg.list_idle();
        assert_eq!(idle.len(), 1);
        assert_eq!(idle[0].id, "a2");
    }

    #[test]
    fn test_find_by_skill() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec!["search", "summarize"]));
        reg.register(make_agent("a2", vec!["write"]));
        reg.register(make_agent("a3", vec!["search"]));

        let searchers = reg.find_by_skill("search");
        assert_eq!(searchers.len(), 2);

        let writers = reg.find_by_skill("write");
        assert_eq!(writers.len(), 1);

        let none = reg.find_by_skill("nonexistent");
        assert!(none.is_empty());
    }

    #[test]
    fn test_get_with_version() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec!["search"]));

        let (agent, version) = reg.get_with_version("a1").unwrap();
        assert_eq!(agent.id, "a1");
        assert_eq!(version, 0);

        assert!(reg.get_with_version("nope").is_none());
    }

    #[test]
    fn test_update_config_success() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec!["search"]));

        let mut updated = make_agent("a1", vec!["search", "summarize"]);
        updated.name = "Updated Agent".to_string();

        let new_version = reg.update_config("a1", updated, 0).unwrap();
        assert_eq!(new_version, 1);

        let (agent, version) = reg.get_with_version("a1").unwrap();
        assert_eq!(agent.name, "Updated Agent");
        assert_eq!(agent.skills.len(), 2);
        assert_eq!(version, 1);
    }

    #[test]
    fn test_update_config_conflict() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec!["search"]));

        // First update succeeds
        let updated = make_agent("a1", vec!["search", "summarize"]);
        reg.update_config("a1", updated, 0).unwrap();

        // Second update with stale version fails
        let updated2 = make_agent("a1", vec!["write"]);
        let err = reg.update_config("a1", updated2, 0).unwrap_err();
        assert_eq!(err, "CONFIG_CONFLICT");
    }

    #[test]
    fn test_update_config_not_found() {
        let reg = AgentRegistry::new();
        let agent = make_agent("a1", vec![]);
        let err = reg.update_config("a1", agent, 0).unwrap_err();
        assert_eq!(err, "AGENT_NOT_FOUND");
    }

    #[test]
    fn test_update_config_sequential_versions() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec![]));

        let v1 = reg
            .update_config("a1", make_agent("a1", vec!["s1"]), 0)
            .unwrap();
        assert_eq!(v1, 1);

        let v2 = reg
            .update_config("a1", make_agent("a1", vec!["s2"]), 1)
            .unwrap();
        assert_eq!(v2, 2);

        let v3 = reg
            .update_config("a1", make_agent("a1", vec!["s3"]), 2)
            .unwrap();
        assert_eq!(v3, 3);
    }

    #[test]
    fn test_try_claim_success() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec!["search"]));

        let agent = reg.try_claim("a1", "task-1".to_string()).unwrap();
        assert_eq!(agent.id, "a1");
        assert_eq!(agent.status.as_str(), "busy");
        assert_eq!(agent.current_task.as_deref(), Some("task-1"));

        // Registry state is also Busy
        let fetched = reg.get("a1").unwrap();
        assert_eq!(fetched.status.as_str(), "busy");
        assert_eq!(fetched.current_task.as_deref(), Some("task-1"));
    }

    #[test]
    fn test_try_claim_already_busy() {
        let reg = AgentRegistry::new();
        reg.register(make_agent("a1", vec!["search"]));

        // First claim succeeds
        reg.try_claim("a1", "task-1".to_string()).unwrap();

        // Second claim fails
        let err = reg.try_claim("a1", "task-2".to_string()).unwrap_err();
        assert!(err.contains("not available"));

        // Original task_id preserved
        let fetched = reg.get("a1").unwrap();
        assert_eq!(fetched.current_task.as_deref(), Some("task-1"));
    }

    #[test]
    fn test_try_claim_not_found() {
        let reg = AgentRegistry::new();
        let err = reg
            .try_claim("nonexistent", "task-1".to_string())
            .unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn test_try_claim_concurrent() {
        use std::sync::Arc;
        use std::thread;

        let reg = Arc::new(AgentRegistry::new());
        reg.register(make_agent("a1", vec!["search"]));

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let reg = reg.clone();
                thread::spawn(move || reg.try_claim("a1", format!("task-{}", i)))
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wins = results.iter().filter(|r| r.is_ok()).count();
        let losses = results.iter().filter(|r| r.is_err()).count();

        assert_eq!(wins, 1, "Exactly one thread should win the claim");
        assert_eq!(losses, 9);
    }

    // ── Template + Instance tests ──────────────────────────────────

    #[test]
    fn test_register_template() {
        let reg = AgentRegistry::new();
        let t = make_template("code_agent", vec!["file_read", "file_write"], false);
        assert!(reg.register_template(t.clone()));
        assert!(!reg.register_template(t)); // duplicate
        assert_eq!(reg.template_count(), 1);

        let fetched = reg.get_template("code_agent").unwrap();
        assert_eq!(fetched.frontmatter.id, "code_agent");
    }

    #[test]
    fn test_list_templates() {
        let reg = AgentRegistry::new();
        reg.register_template(make_template("a", vec![], false));
        reg.register_template(make_template("b", vec![], false));
        assert_eq!(reg.list_templates().len(), 2);
    }

    #[test]
    fn test_find_templates_by_skill() {
        let reg = AgentRegistry::new();
        reg.register_template(make_template("a", vec!["search"], false));
        reg.register_template(make_template("b", vec!["write"], false));
        reg.register_template(make_template("c", vec!["search", "write"], false));

        let searchers = reg.find_templates_by_skill("search");
        assert_eq!(searchers.len(), 2);

        let writers = reg.find_templates_by_skill("write");
        assert_eq!(writers.len(), 2);

        let none = reg.find_templates_by_skill("nonexistent");
        assert!(none.is_empty());
    }

    #[test]
    fn test_spawn_instance_non_singleton() {
        let reg = AgentRegistry::new();
        reg.register_template(make_template("code_agent", vec!["file_read"], false));

        let inst1 = reg.spawn_instance("code_agent", "task-1".into()).unwrap();
        assert!(inst1.id.starts_with("code_agent::"));
        assert_eq!(inst1.template_id, "code_agent");
        assert_eq!(inst1.status.as_str(), "busy");
        assert_eq!(inst1.current_task.as_deref(), Some("task-1"));

        // Can spawn multiple instances from the same template
        let inst2 = reg.spawn_instance("code_agent", "task-2".into()).unwrap();
        assert!(inst2.id.starts_with("code_agent::"));
        assert_ne!(inst1.id, inst2.id);
        assert_eq!(reg.count(), 2);
    }

    #[test]
    fn test_spawn_instance_singleton() {
        let reg = AgentRegistry::new();
        reg.register_template(make_template("lead_agent", vec!["orchestrate"], true));

        // First spawn creates the singleton instance
        let inst1 = reg.spawn_instance("lead_agent", "task-1".into()).unwrap();
        assert_eq!(inst1.id, "lead_agent"); // stable ID
        assert_eq!(inst1.status.as_str(), "busy");

        // Second spawn fails (singleton is busy)
        let err = reg
            .spawn_instance("lead_agent", "task-2".into())
            .unwrap_err();
        assert!(err.contains("busy"));

        // Release singleton
        reg.destroy_instance("lead_agent");
        let fetched = reg.get_instance("lead_agent").unwrap();
        assert!(fetched.status.is_available());

        // Re-claim succeeds
        let inst2 = reg.spawn_instance("lead_agent", "task-3".into()).unwrap();
        assert_eq!(inst2.id, "lead_agent");
        assert_eq!(inst2.current_task.as_deref(), Some("task-3"));
    }

    #[test]
    fn test_spawn_instance_template_not_found() {
        let reg = AgentRegistry::new();
        let err = reg.spawn_instance("nope", "task-1".into()).unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn test_destroy_instance_non_singleton() {
        let reg = AgentRegistry::new();
        reg.register_template(make_template("code_agent", vec![], false));

        let inst = reg.spawn_instance("code_agent", "task-1".into()).unwrap();
        assert_eq!(reg.count(), 1);

        // Destroying non-singleton removes it entirely
        assert_eq!(reg.destroy_instance(&inst.id), DestroyOutcome::Removed);
        assert_eq!(reg.count(), 0);
        assert!(reg.get_instance(&inst.id).is_none());
    }

    #[test]
    fn test_destroy_instance_singleton_resets_to_idle() {
        let reg = AgentRegistry::new();
        reg.register_template(make_template("lead", vec![], true));

        let inst = reg.spawn_instance("lead", "task-1".into()).unwrap();
        assert_eq!(inst.id, "lead");

        // Destroying singleton resets to Idle instead of removing
        reg.destroy_instance("lead");
        let fetched = reg.get_instance("lead").unwrap();
        assert!(fetched.status.is_available());
        assert!(fetched.current_task.is_none());
        assert_eq!(reg.count(), 1); // still in registry
    }

    #[test]
    fn test_count_instances_of() {
        let reg = AgentRegistry::new();
        reg.register_template(make_template("code_agent", vec![], false));
        reg.register_template(make_template("research_agent", vec![], false));

        reg.spawn_instance("code_agent", "t1".into()).unwrap();
        reg.spawn_instance("code_agent", "t2".into()).unwrap();
        reg.spawn_instance("research_agent", "t3".into()).unwrap();

        assert_eq!(reg.count_instances_of("code_agent"), 2);
        assert_eq!(reg.count_instances_of("research_agent"), 1);
        assert_eq!(reg.count_instances_of("nonexistent"), 0);
    }

    #[test]
    fn test_spawn_instance_concurrent() {
        use std::sync::Arc;
        use std::thread;

        let reg = Arc::new(AgentRegistry::new());
        reg.register_template(make_template("lead", vec![], true));

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let reg = reg.clone();
                thread::spawn(move || reg.spawn_instance("lead", format!("task-{}", i)))
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wins = results.iter().filter(|r| r.is_ok()).count();
        let losses = results.iter().filter(|r| r.is_err()).count();

        assert_eq!(wins, 1, "Exactly one thread should win the singleton claim");
        assert_eq!(losses, 9);
    }

    #[test]
    fn test_spawn_non_singleton_concurrent() {
        use std::sync::Arc;
        use std::thread;

        let reg = Arc::new(AgentRegistry::new());
        reg.register_template(make_template("worker", vec![], false));

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let reg = reg.clone();
                thread::spawn(move || reg.spawn_instance("worker", format!("task-{}", i)))
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wins = results.iter().filter(|r| r.is_ok()).count();

        // All 10 should succeed for non-singleton
        assert_eq!(wins, 10, "All threads should spawn non-singleton instances");
        assert_eq!(reg.count(), 10);
    }

    #[test]
    fn test_template_with_markdown() {
        let md = r#"---
id: "test_agent"
name: "Test Agent"
description: "A test agent"
singleton: false
skills:
  - "file_read"
temperature: 0.3
---

## Persona

You are a test agent.
"#;
        let template = parse_agent_markdown(md).unwrap();
        let reg = AgentRegistry::new();
        reg.register_template(template);

        let inst = reg.spawn_instance("test_agent", "task-1".into()).unwrap();
        assert!(inst.id.starts_with("test_agent::"));
        assert_eq!(inst.preset.persona, "You are a test agent.");
        assert_eq!(inst.preset.temperature, 0.3);
        assert_eq!(inst.skills.len(), 1);
        assert_eq!(inst.skills[0].name, "file_read");
    }
