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

#[test]
fn test_validate_smoke_no_tests_defined() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(
        tmp.path(),
        "no-tests",
        r#"---
name: "No Tests"
description: "Skill with no smoke tests"
---
"#,
    );

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    let results = validate_smoke_config("no-tests", &catalog);
    assert!(results.is_empty());
}

#[test]
fn test_validate_smoke_missing_input_file() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(
        tmp.path(),
        "with-smoke",
        r#"---
name: "With Smoke"
description: "Skill with smoke test referencing missing file"
tests:
  smoke:
    - "test_input.txt"
---
"#,
    );

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    let results = validate_smoke_config("with-smoke", &catalog);
    assert_eq!(results.len(), 1);
    assert!(!results[0].passed);
    assert!(results[0].error.as_ref().unwrap().contains("not found"));
}

#[test]
fn test_validate_smoke_input_file_exists() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = create_skill_dir(
        tmp.path(),
        "valid-smoke",
        r#"---
name: "Valid Smoke"
description: "Skill with valid smoke test"
tests:
  smoke:
    - "test_input.txt"
---
"#,
    );

    // Create the input file
    std::fs::write(skill_dir.join("test_input.txt"), "review this code").unwrap();

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    let results = validate_smoke_config("valid-smoke", &catalog);
    assert_eq!(results.len(), 1);
    assert!(results[0].passed);
    assert!(results[0].error.is_none());
}

#[test]
fn test_validate_smoke_skill_not_found() {
    let catalog = SkillCatalog::new();
    let results = validate_smoke_config("nonexistent", &catalog);
    assert_eq!(results.len(), 1);
    assert!(!results[0].passed);
    assert!(results[0].error.as_ref().unwrap().contains("not found"));
}

#[test]
fn test_check_expected_output_all_present() {
    let output = "## Critical\nFound a bug.\n## Summary\nOverall looks good.";
    let expected = vec!["Critical".to_string(), "Summary".to_string()];
    let missing = check_expected_output(output, &expected);
    assert!(missing.is_empty());
}

#[test]
fn test_check_expected_output_some_missing() {
    let output = "## Summary\nLooks fine.";
    let expected = vec![
        "Critical".to_string(),
        "Summary".to_string(),
        "Performance".to_string(),
    ];
    let missing = check_expected_output(output, &expected);
    assert_eq!(missing.len(), 2);
    assert!(missing.contains(&"Critical".to_string()));
    assert!(missing.contains(&"Performance".to_string()));
}

#[test]
fn test_check_expected_output_empty_expectations() {
    let output = "anything";
    let missing = check_expected_output(output, &[]);
    assert!(missing.is_empty());
}

#[test]
fn test_migrated_code_review_parses() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(
        tmp.path(),
        "code-review",
        r#"---
id: code-review
name: Code Review
version: 0.1.0
description: Review code for bugs, style issues, security concerns, and improvements
invoke:
  mode: auto
  slash: /review
routing:
  intent:
    - "review code"
    - "code review"
  keywords:
    - "review"
    - "bugs"
  negative_keywords:
    - "write"
permissions:
  level: readonly
tools:
  allow:
    - "file_read"
output:
  format: markdown
  required_sections:
    - "Critical"
    - "Summary"
---

## Instructions

Review the code.
"#,
    );

    let catalog = SkillCatalog::new();
    let count = catalog.scan_directory(tmp.path(), SkillScope::Project);
    assert_eq!(count, 1);

    let entry = catalog.get("code-review").unwrap();
    assert_eq!(entry.frontmatter.name, "Code Review");
    assert_eq!(entry.frontmatter.version, Some("0.1.0".to_string()));
    assert_eq!(entry.frontmatter.invoke.mode, "auto");
    assert_eq!(entry.frontmatter.invoke.slash, Some("/review".to_string()));
    assert_eq!(
        entry.frontmatter.routing.intent,
        vec!["review code", "code review"]
    );
    assert_eq!(entry.frontmatter.routing.keywords, vec!["review", "bugs"]);
    assert_eq!(entry.frontmatter.routing.negative_keywords, vec!["write"]);
    assert_eq!(entry.frontmatter.permissions.level, "readonly");
    assert_eq!(entry.frontmatter.tools.allow, vec!["file_read"]);
    assert_eq!(
        entry.frontmatter.output.required_sections,
        vec!["Critical", "Summary"]
    );

    // Verify full document loads
    let doc = catalog.load_full("code-review").unwrap();
    assert!(doc.body.contains("Review the code"));
}

#[test]
fn test_migrated_commit_message_parses() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(
        tmp.path(),
        "commit-message",
        r#"---
id: commit-message
name: Commit Message
version: 0.1.0
description: Generate conventional commit messages from code changes
invoke:
  mode: auto
  slash: /commit
routing:
  intent:
    - "commit message"
    - "git commit"
  keywords:
    - "commit"
    - "git"
  negative_keywords:
    - "review"
permissions:
  level: write_repo
  sandbox:
    allow_shell: true
tools:
  allow:
    - "shell_execute"
output:
  format: markdown
---

## Instructions

Generate a commit message.
"#,
    );

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    let entry = catalog.get("commit-message").unwrap();
    assert_eq!(entry.frontmatter.name, "Commit Message");
    assert_eq!(entry.frontmatter.invoke.mode, "auto");
    assert_eq!(entry.frontmatter.invoke.slash, Some("/commit".to_string()));
    assert_eq!(entry.frontmatter.permissions.level, "write_repo");
    assert_eq!(entry.frontmatter.tools.allow, vec!["shell_execute"]);
}

#[test]
fn test_migrated_explain_code_parses() {
    let tmp = TempDir::new().unwrap();
    create_skill_dir(
        tmp.path(),
        "explain-code",
        r#"---
id: explain-code
name: Explain Code
version: 0.1.0
description: Explain what a piece of code does in plain language
invoke:
  mode: auto
  slash: /explain-code
routing:
  intent:
    - "explain code"
    - "what does this do"
  keywords:
    - "explain"
    - "understand"
  negative_keywords:
    - "write"
    - "review"
permissions:
  level: readonly
tools:
  allow:
    - "file_read"
output:
  format: markdown
---

## Instructions

Explain the code.
"#,
    );

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);

    let entry = catalog.get("explain-code").unwrap();
    assert_eq!(entry.frontmatter.name, "Explain Code");
    assert_eq!(entry.frontmatter.invoke.mode, "auto");
    assert_eq!(
        entry.frontmatter.invoke.slash,
        Some("/explain-code".to_string())
    );
    assert_eq!(entry.frontmatter.permissions.level, "readonly");
    assert_eq!(
        entry.frontmatter.routing.negative_keywords,
        vec!["write", "review"]
    );
}
