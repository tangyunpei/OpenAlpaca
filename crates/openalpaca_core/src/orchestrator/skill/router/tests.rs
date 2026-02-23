    use super::*;
    use crate::middleware::skill::SkillScope;
    use std::io::Write;
    use std::path::Path;
    use tempfile::TempDir;

    fn create_skill_dir(parent: &Path, name: &str, skill_md: &str) -> std::path::PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let md_path = dir.join("SKILL.md");
        let mut f = std::fs::File::create(&md_path).unwrap();
        f.write_all(skill_md.as_bytes()).unwrap();
        dir
    }

    fn default_router() -> SkillRouter {
        SkillRouter::new(0.65, 0.45)
    }

    #[test]
    fn test_intent_match_auto_select() {
        // intent match alone: base(0.2) + intent(0.45) = 0.65 -> auto-select
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "code-review",
            r#"---
name: "Code Review"
description: "Review code"
invoke:
  mode: auto
routing:
  intent:
    - "review code"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("please review code for me", &catalog);

        assert!(result.selected.is_some());
        assert_eq!(result.selected.unwrap(), "code-review");
    }

    #[test]
    fn test_keywords_only_below_threshold() {
        // 2/4 keywords: base(0.2) + 0.5 * 0.35 = 0.375 -> skip
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "data-analysis",
            r#"---
name: "Data Analysis"
description: "Analyze data"
invoke:
  mode: auto
routing:
  keywords:
    - "data"
    - "analyze"
    - "statistics"
    - "chart"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("analyze the data", &catalog);

        // Score 0.375 < 0.45 suggest threshold -> no selected, no suggestions
        assert!(result.selected.is_none());
        assert!(result.suggestions.is_empty());
        assert_eq!(result.scores.len(), 1);
        let score = &result.scores[0];
        assert!((score.score - 0.375).abs() < 0.01);
    }

    #[test]
    fn test_intent_plus_keywords_full_score() {
        // intent + all keywords: 0.2 + 0.45 + 1.0 * 0.35 = 1.0 -> auto-select
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "review",
            r#"---
name: "Full Review"
description: "Full review"
invoke:
  mode: auto
routing:
  intent:
    - "review"
  keywords:
    - "code"
    - "bugs"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("review code for bugs", &catalog);

        assert!(result.selected.is_some());
        let score = &result.scores[0];
        assert!((score.score - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_negative_keyword_penalty() {
        // intent match: 0.2 + 0.45 = 0.65, minus 0.6 penalty = 0.05
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "deploy",
            r#"---
name: "Deploy"
description: "Deploy code"
invoke:
  mode: auto
routing:
  intent:
    - "deploy"
  negative_keywords:
    - "rollback"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("deploy but also rollback if needed", &catalog);

        assert!(result.selected.is_none());
        assert!(result.suggestions.is_empty());
        let score = &result.scores[0];
        assert!(score.negative_hit);
        assert!((score.score - 0.05).abs() < 0.01);
    }

    #[test]
    fn test_recency_bonus() {
        // base(0.2) + recency(0.2) = 0.4, still below suggest threshold
        // With intent: 0.2 + 0.45 + 0.2 = 0.85 -> auto-select
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "review",
            r#"---
name: "Review"
description: "Review"
invoke:
  mode: auto
routing:
  intent:
    - "review"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        router.record_usage("review");

        let result = router.route("review this", &catalog);
        let score = &result.scores[0];
        assert!((score.recency_bonus - 1.0).abs() < f64::EPSILON);
        assert!((score.score - 0.85).abs() < 0.01);
    }

    #[test]
    fn test_no_routing_config_base_only() {
        // No routing config -> score = base(0.2) -> skip
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "empty-routing",
            r#"---
name: "Empty"
description: "No routing"
invoke:
  mode: auto
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("anything at all", &catalog);

        assert!(result.selected.is_none());
        assert!(result.suggestions.is_empty());
        let score = &result.scores[0];
        assert!((score.score - 0.2).abs() < 0.01);
    }

    #[test]
    fn test_disabled_skill_excluded() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "disabled",
            r#"---
name: "Disabled"
description: "Disabled skill"
invoke:
  mode: disabled
routing:
  intent:
    - "anything"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("anything", &catalog);

        // Disabled skill is not even in the catalog (filtered at scan time)
        assert!(result.scores.is_empty());
    }

    #[test]
    fn test_multiple_skills_highest_wins() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "code-review",
            r#"---
name: "Code Review"
description: "Review code"
invoke:
  mode: auto
routing:
  intent:
    - "review code"
  keywords:
    - "bugs"
    - "style"
---
"#,
        );
        create_skill_dir(
            tmp.path(),
            "explain",
            r#"---
name: "Explain"
description: "Explain code"
invoke:
  mode: auto
routing:
  intent:
    - "explain"
  keywords:
    - "understand"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("review code for bugs and style issues", &catalog);

        // code-review: intent(0.45) + keywords(2/2 * 0.35) + base(0.2) = 1.0
        // explain: base only = 0.2
        assert_eq!(result.selected, Some("code-review".to_string()));
        assert_eq!(result.scores[0].skill_id, "code-review");
    }

    #[test]
    fn test_manual_mode_not_auto_selected() {
        // Score >= 0.65 but mode is "manual" -> goes to suggestions, not selected
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "manual-skill",
            r#"---
name: "Manual Skill"
description: "Manual only"
invoke:
  mode: manual
routing:
  intent:
    - "manual trigger"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("manual trigger please", &catalog);

        // Score is 0.65 (intent match) but mode is manual -> suggestion only
        assert!(result.selected.is_none());
        assert_eq!(result.suggestions.len(), 1);
        assert_eq!(result.suggestions[0].skill_id, "manual-skill");
    }

    #[test]
    fn test_record_usage_recency_window() {
        let router = SkillRouter::new(0.65, 0.45);

        // Fill beyond the window
        for i in 0..15 {
            router.record_usage(&format!("skill-{}", i));
        }

        let recent = router.recent_skills.read().unwrap();
        assert_eq!(recent.len(), DEFAULT_RECENCY_WINDOW);
        // Oldest entries should be evicted
        assert!(!recent.contains(&"skill-0".to_string()));
        assert!(recent.contains(&"skill-14".to_string()));
    }

    #[test]
    fn test_record_usage_dedup() {
        let router = SkillRouter::new(0.65, 0.45);
        router.record_usage("skill-a");
        router.record_usage("skill-b");
        router.record_usage("skill-a"); // should move to end, not duplicate

        let recent = router.recent_skills.read().unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0], "skill-b");
        assert_eq!(recent[1], "skill-a");
    }

    #[test]
    fn test_suggest_range() {
        // Score 0.45..0.65 goes to suggestions
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "partial-match",
            r#"---
name: "Partial Match"
description: "Partially matches"
invoke:
  mode: auto
routing:
  keywords:
    - "data"
    - "analysis"
    - "chart"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        // 2/3 keywords matched: 0.2 + (2/3) * 0.35 = 0.433... -> below suggest
        // 3/3 keywords: 0.2 + 1.0 * 0.35 = 0.55 -> in suggest range
        let result = router.route("data analysis chart", &catalog);

        // All 3 keywords match: score = 0.55
        assert!(result.selected.is_none()); // below 0.65
        assert_eq!(result.suggestions.len(), 1);
        assert!((result.suggestions[0].score - 0.55).abs() < 0.01);
    }
