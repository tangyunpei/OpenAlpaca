use super::*;
use crate::middleware::skill::SkillScope;
use std::io::Write;
use tempfile::TempDir;

fn create_skill_dir(parent: &Path, name: &str, skill_md: &str) -> PathBuf {
    let dir = parent.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let md_path = dir.join("SKILL.md");
    let mut f = std::fs::File::create(&md_path).unwrap();
    f.write_all(skill_md.as_bytes()).unwrap();
    dir
}

const REVIEW_SKILL: &str = r#"---
name: "Code Review"
description: "Review code for bugs and style issues"
command: "review"
trigger_patterns:
  - "review.*code"
  - "code review"
tools_required:
  - "file_read"
auto_load: false
read_when:
  - "User asks for code review"
---

## Instructions

Analyze the code for bugs and style.
"#;

const EXPLAIN_SKILL: &str = r#"---
name: "Explain Code"
description: "Explain what code does"
command: "explain-code"
trigger_patterns:
  - "explain.*code"
  - "what does.*do"
auto_load: false
---

## Instructions

Walk through the code step by step.
"#;

#[test]
fn test_scan_directory() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);
    create_skill_dir(tmp.path(), "explain-code", EXPLAIN_SKILL);

    let catalog = SkillCatalog::new();
    let count = catalog.scan_directory(tmp.path(), SkillScope::Project);
    assert_eq!(count, 2);
    assert_eq!(catalog.count(), 2);
}

#[test]
fn test_get_by_id() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    // Lookup by directory name (skill ID)
    let entry = catalog.get("code-review").expect("should find by ID");
    assert_eq!(entry.frontmatter.name, "Code Review");
    assert_eq!(entry.scope, SkillScope::Project);
}

#[test]
fn test_get_by_name_fallback() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    // Lookup by frontmatter name (fallback)
    let entry = catalog.get("Code Review").expect("should find by name");
    assert_eq!(entry.frontmatter.name, "Code Review");
    assert_eq!(entry.frontmatter.command, Some("review".to_string()));

    // Case insensitive
    let entry2 = catalog
        .get("code review")
        .expect("should find case-insensitive");
    assert_eq!(entry2.frontmatter.name, "Code Review");

    assert!(catalog.get("nonexistent").is_none());
}

#[test]
fn test_get_by_command() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);
    create_skill_dir(tmp.path(), "explain-code", EXPLAIN_SKILL);

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    let entry = catalog
        .get_by_command("review")
        .expect("should find /review");
    assert_eq!(entry.frontmatter.name, "Code Review");

    let entry2 = catalog
        .get_by_command("explain-code")
        .expect("should find /explain-code");
    assert_eq!(entry2.frontmatter.name, "Explain Code");

    assert!(catalog.get_by_command("nonexistent").is_none());
}

#[test]
fn test_match_triggers() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);
    create_skill_dir(tmp.path(), "explain-code", EXPLAIN_SKILL);

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    let matches = catalog.match_triggers("please review my code");
    assert!(!matches.is_empty());
    assert!(matches.contains(&"code-review".to_string()));

    let matches2 = catalog.match_triggers("explain this code to me");
    assert!(!matches2.is_empty());
    assert!(matches2.contains(&"explain-code".to_string()));

    let matches3 = catalog.match_triggers("hello world");
    assert!(matches3.is_empty());
}

#[test]
fn test_load_full() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    let doc = catalog
        .load_full("code-review")
        .expect("should load full by ID");
    assert_eq!(doc.frontmatter.name, "Code Review");
    assert!(doc.body.contains("Analyze the code"));
    assert!(doc.sections.contains_key("Instructions"));

    // Also works by name fallback
    let doc2 = catalog
        .load_full("Code Review")
        .expect("should load full by name");
    assert_eq!(doc2.frontmatter.name, "Code Review");
}

#[test]
fn test_load_full_not_found() {
    let catalog = SkillCatalog::new();
    let result = catalog.load_full("nonexistent");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[test]
fn test_catalog_summary() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);
    create_skill_dir(tmp.path(), "explain-code", EXPLAIN_SKILL);

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    let summary = catalog.catalog_summary();
    assert_eq!(summary.len(), 2);
    // Sorted by name
    assert_eq!(summary[0].0, "Code Review");
    assert_eq!(summary[1].0, "Explain Code");
    assert_eq!(summary[0].2, Some("review".to_string()));
}

#[test]
fn test_remove() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);
    assert_eq!(catalog.count(), 1);
    assert!(catalog.get_by_command("review").is_some());

    // Remove by ID
    catalog.remove("code-review");
    assert_eq!(catalog.count(), 0);
    assert!(catalog.get_by_command("review").is_none());
}

#[test]
fn test_remove_by_name() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);
    assert_eq!(catalog.count(), 1);

    // Remove by frontmatter name (backward compat)
    catalog.remove("Code Review");
    assert_eq!(catalog.count(), 0);
}

#[test]
fn test_reload_skill() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    let entry = catalog.get("code-review").unwrap();
    assert_eq!(
        entry.frontmatter.description,
        "Review code for bugs and style issues"
    );

    // Overwrite with updated content
    let updated = REVIEW_SKILL.replace(
        "Review code for bugs and style issues",
        "Updated description",
    );
    std::fs::write(skill_dir.join("SKILL.md"), updated).unwrap();

    catalog
        .reload_skill(&skill_dir)
        .expect("reload should succeed");

    let entry2 = catalog.get("code-review").unwrap();
    assert_eq!(entry2.frontmatter.description, "Updated description");
}

#[test]
fn test_scan_ignores_non_directories() {
    let tmp = TempDir::new().unwrap();
    // Create a regular file (not a directory) in the skills dir
    std::fs::write(tmp.path().join("not_a_dir.md"), "hello").unwrap();
    create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

    let catalog = SkillCatalog::new();
    let count = catalog.scan_directory(tmp.path(), SkillScope::Project);
    assert_eq!(count, 1);
}

#[test]
fn test_scan_ignores_dirs_without_skill_md() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("empty-dir")).unwrap();
    create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

    let catalog = SkillCatalog::new();
    let count = catalog.scan_directory(tmp.path(), SkillScope::Project);
    assert_eq!(count, 1);
}

#[test]
fn test_empty_catalog() {
    let catalog = SkillCatalog::new();
    assert!(catalog.is_empty());
    assert_eq!(catalog.count(), 0);
    assert!(catalog.list_names().is_empty());
    assert!(catalog.catalog_summary().is_empty());
    assert!(catalog.match_triggers("anything").is_empty());
}

#[test]
fn test_multi_scope_project_overrides_user() {
    let user_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();

    // User has a code-review skill
    create_skill_dir(
        user_dir.path(),
        "code-review",
        r#"---
name: "Code Review (User)"
description: "User-level code review"
command: "review"
---
"#,
    );

    // Project overrides with its own code-review
    create_skill_dir(
        project_dir.path(),
        "code-review",
        r#"---
name: "Code Review (Project)"
description: "Project-level code review"
command: "review"
---
"#,
    );

    let catalog = SkillCatalog::new();
    catalog.scan_multi_scope(Some(user_dir.path()), Some(project_dir.path()));

    // Should have 1 skill (project overrides user)
    assert_eq!(catalog.count(), 1);
    let entry = catalog.get("code-review").unwrap();
    assert_eq!(entry.frontmatter.name, "Code Review (Project)");
    assert_eq!(entry.scope, SkillScope::Project);
}

#[test]
fn test_multi_scope_user_only_when_no_project_override() {
    let user_dir = TempDir::new().unwrap();
    let project_dir = TempDir::new().unwrap();

    // User has unique skill
    create_skill_dir(
        user_dir.path(),
        "my-skill",
        r#"---
name: "My User Skill"
description: "A user-specific skill"
---
"#,
    );

    // Project has a different skill
    create_skill_dir(project_dir.path(), "code-review", REVIEW_SKILL);

    let catalog = SkillCatalog::new();
    catalog.scan_multi_scope(Some(user_dir.path()), Some(project_dir.path()));

    assert_eq!(catalog.count(), 2);
    let user_entry = catalog.get("my-skill").unwrap();
    assert_eq!(user_entry.scope, SkillScope::User);
    let proj_entry = catalog.get("code-review").unwrap();
    assert_eq!(proj_entry.scope, SkillScope::Project);
}

#[test]
fn test_disabled_skill_skipped() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(
        tmp.path(),
        "disabled-skill",
        r#"---
name: "Disabled Skill"
description: "This skill is disabled"
invoke:
  mode: disabled
---
"#,
    );
    create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

    let catalog = SkillCatalog::new();
    let count = catalog.scan_directory(tmp.path(), SkillScope::Project);
    // disabled skill is skipped but still "processed" (returns Ok from load_entry)
    assert_eq!(count, 2);
    // Only the enabled skill is in the catalog
    assert_eq!(catalog.count(), 1);
    assert!(catalog.get("code-review").is_some());
    assert!(catalog.get("disabled-skill").is_none());
}

#[test]
fn test_id_mismatch_warning() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(
        tmp.path(),
        "actual-dir-name",
        r#"---
id: "different-id"
name: "Mismatched Skill"
description: "ID does not match directory"
---
"#,
    );

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    // Skill is still loaded (just a warning)
    assert_eq!(catalog.count(), 1);
    let errors = catalog.validation_errors();
    assert!(!errors.is_empty());
    assert!(errors[0].contains("does not match directory name"));
}

#[test]
fn test_alias_lookup() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(
        tmp.path(),
        "code-review",
        r#"---
name: "Code Review"
description: "Review code"
invoke:
  slash: "/review"
  aliases:
    - "/cr"
    - "/code-review"
---
"#,
    );

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    // Primary command works
    let entry = catalog.get_by_command("review").expect("primary command");
    assert_eq!(entry.frontmatter.name, "Code Review");

    // Aliases work
    let entry2 = catalog.get_by_command("cr").expect("alias cr");
    assert_eq!(entry2.frontmatter.name, "Code Review");

    let entry3 = catalog
        .get_by_command("code-review")
        .expect("alias code-review");
    assert_eq!(entry3.frontmatter.name, "Code Review");
}

#[test]
fn test_slash_conflict_produces_validation_error() {
    let tmp = TempDir::new().unwrap();
    // Two skills claiming the same slash command
    create_skill_dir(
        tmp.path(),
        "skill-a",
        r#"---
name: "Skill A"
description: "First skill"
command: "review"
---
"#,
    );
    create_skill_dir(
        tmp.path(),
        "skill-b",
        r#"---
name: "Skill B"
description: "Second skill"
command: "review"
---
"#,
    );

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    // Both loaded but one overwrites the command
    assert_eq!(catalog.count(), 2);
    let errors = catalog.validation_errors();
    assert!(
        errors.iter().any(|e| e.contains("conflict")),
        "Expected slash conflict warning, got: {:?}",
        errors
    );
}

#[test]
fn test_scope_field_preserved() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(tmp.path(), "code-review", REVIEW_SKILL);

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::User);

    let entry = catalog.get("code-review").unwrap();
    assert_eq!(entry.scope, SkillScope::User);
}

// ===========================================================================
// Task 13: SkillCatalog concurrent access tests
// ===========================================================================

#[tokio::test]
async fn test_concurrent_match_triggers() {
    let tmp = TempDir::new().unwrap();

    // Create 5 skill directories with trigger patterns
    for i in 0..5 {
        let skill_md = format!(
            r#"---
name: "Skill {i}"
description: "Skill number {i}"
trigger_patterns:
  - "query.*{i}"
---

## Instructions

Do something for skill {i}.
"#
        );
        create_skill_dir(tmp.path(), &format!("skill-{i}"), &skill_md);
    }

    let catalog = Arc::new(SkillCatalog::new());
    catalog.scan_directory(tmp.path(), SkillScope::Project);
    assert_eq!(catalog.count(), 5);

    // Spawn 10 tasks calling match_triggers concurrently
    let mut handles = Vec::new();
    for _ in 0..10 {
        let cat = catalog.clone();
        handles.push(tokio::spawn(async move {
            let _ = cat.match_triggers("query about 2");
        }));
    }

    for h in handles {
        h.await.unwrap();
    }
    // Test completes without panics — concurrent reads are safe
}

#[tokio::test]
async fn test_concurrent_register_plugin_skill() {
    use openalpaca_api::plugin_traits::{PluginSkillExecutor, ToolCallbackExecutor};

    struct MockSkillExecutor {
        id: String,
    }
    #[async_trait::async_trait]
    impl PluginSkillExecutor for MockSkillExecutor {
        async fn invoke(
            &self,
            _q: &str,
            _c: &serde_json::Value,
            _t: &dyn ToolCallbackExecutor,
        ) -> Result<String, String> {
            Ok("ok".into())
        }
        fn plugin_id(&self) -> &str {
            "test"
        }
        fn skill_id(&self) -> &str {
            &self.id
        }
    }

    let catalog = Arc::new(SkillCatalog::new());
    let mut handles = Vec::new();

    // Spawn 5 tasks each registering a different plugin skill
    for i in 0..5 {
        let cat = catalog.clone();
        handles.push(tokio::spawn(async move {
            let skill_id = format!("plugin-skill-{i}");
            let frontmatter = crate::middleware::skill::SkillFrontmatter {
                name: format!("Plugin Skill {i}"),
                description: format!("Description for plugin skill {i}"),
                ..Default::default()
            };
            let executor = Arc::new(MockSkillExecutor {
                id: skill_id.clone(),
            });
            cat.register_plugin_skill(
                skill_id,
                frontmatter,
                executor,
                "test-plugin".to_string(),
            );
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // Verify all 5 exist
    assert_eq!(catalog.count(), 5);
    for i in 0..5 {
        let entry = catalog.get(&format!("plugin-skill-{i}"));
        assert!(
            entry.is_some(),
            "plugin-skill-{} should exist in catalog",
            i
        );
    }
}
