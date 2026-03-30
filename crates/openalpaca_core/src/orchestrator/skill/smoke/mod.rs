//! Smoke test runner for skill testing.
//!
//! Validates skill test configurations and provides infrastructure
//! for running smoke tests against skill invocations.

use crate::orchestrator::skill::catalog::SkillCatalog;

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
    let skill_dir = match entry.skill_dir {
        Some(ref d) => d,
        None => {
            // Plugin skills have no filesystem directory — smoke tests are not applicable
            return Vec::new();
        }
    };
    for input_file in &doc.frontmatter.tests.smoke {
        let full_path = skill_dir.join(input_file);
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
mod tests;
