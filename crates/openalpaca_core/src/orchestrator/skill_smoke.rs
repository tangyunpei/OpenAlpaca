//! Smoke test runner for skill testing.
//!
//! Validates skill test configurations and provides infrastructure
//! for running smoke tests against skill invocations.

use crate::orchestrator::skill_catalog::SkillCatalog;

/// Result of a smoke test validation (not full execution).
#[derive(Debug, Clone)]
pub struct SmokeTestResult {
    pub skill_id: String,
    pub input_file: String,
    pub passed: bool,
    pub missing_keywords: Vec<String>,
    pub output_tokens: usize,
    pub tool_calls: usize,
    pub error: Option<String>,
}

/// Validate smoke test configuration for a skill.
///
/// Checks that input files exist and expect config is valid.
/// Does NOT execute the skill (that requires an LLM router).
pub fn validate_smoke_config(skill_id: &str, catalog: &SkillCatalog) -> Vec<SmokeTestResult> {
    let entry = match catalog.get(skill_id) {
        Some(e) => e,
        None => {
            return vec![SmokeTestResult {
                skill_id: skill_id.to_string(),
                input_file: String::new(),
                passed: false,
                missing_keywords: Vec::new(),
                output_tokens: 0,
                tool_calls: 0,
                error: Some(format!("Skill '{}' not found in catalog", skill_id)),
            }];
        }
    };

    let doc = match catalog.load_full(skill_id) {
        Ok(d) => d,
        Err(e) => {
            return vec![SmokeTestResult {
                skill_id: skill_id.to_string(),
                input_file: String::new(),
                passed: false,
                missing_keywords: Vec::new(),
                output_tokens: 0,
                tool_calls: 0,
                error: Some(e),
            }];
        }
    };

    if doc.frontmatter.tests.smoke.is_empty() {
        return Vec::new(); // No smoke tests defined
    }

    let mut results = Vec::new();
    for input_file in &doc.frontmatter.tests.smoke {
        let full_path = entry.skill_dir.join(input_file);
        if !full_path.exists() {
            results.push(SmokeTestResult {
                skill_id: skill_id.to_string(),
                input_file: input_file.clone(),
                passed: false,
                missing_keywords: Vec::new(),
                output_tokens: 0,
                tool_calls: 0,
                error: Some(format!("Input file not found: {}", full_path.display())),
            });
        } else {
            // Input file exists — config is valid
            results.push(SmokeTestResult {
                skill_id: skill_id.to_string(),
                input_file: input_file.clone(),
                passed: true,
                missing_keywords: Vec::new(),
                output_tokens: 0,
                tool_calls: 0,
                error: None,
            });
        }
    }
    results
}

/// Check if a skill output passes the `tests.expect.contains` assertions.
///
/// Returns a list of expected keywords that were NOT found in the output.
pub fn check_expected_output(output: &str, expected_contains: &[String]) -> Vec<String> {
    expected_contains
        .iter()
        .filter(|kw| !output.contains(kw.as_str()))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
        assert_eq!(
            entry.frontmatter.invoke.slash,
            Some("/review".to_string())
        );
        assert_eq!(
            entry.frontmatter.routing.intent,
            vec!["review code", "code review"]
        );
        assert_eq!(
            entry.frontmatter.routing.keywords,
            vec!["review", "bugs"]
        );
        assert_eq!(
            entry.frontmatter.routing.negative_keywords,
            vec!["write"]
        );
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
        assert_eq!(
            entry.frontmatter.invoke.slash,
            Some("/commit".to_string())
        );
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
}
