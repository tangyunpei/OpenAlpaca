//! Output validation for skill invocations.
//!
//! Validates skill output against the `output` frontmatter config:
//! - `format: "markdown"` — checks for `required_sections` as `## Heading`
//! - `format: "json"` — validates JSON parse, extracts from code blocks if needed
//! - Warns (but does not reject) if output exceeds `max_tokens`

use crate::middleware::skill::OutputConfig;

/// Errors from output validation.
#[derive(Debug, Clone)]
pub enum OutputValidationError {
    MissingSections(Vec<String>),
    InvalidJson(String),
}

impl OutputValidationError {
    /// Generate a repair prompt that can be appended to the conversation
    /// and re-sent to the LLM for a self-repair attempt.
    pub fn repair_prompt(&self) -> String {
        match self {
            Self::MissingSections(sections) => {
                format!(
                    "Your output is missing required sections: {}. \
                     Please add them as H2 headings (## SectionName).",
                    sections.join(", ")
                )
            }
            Self::InvalidJson(err) => {
                format!(
                    "Your output must be valid JSON. Parse error: {}. \
                     Please output only valid JSON.",
                    err
                )
            }
        }
    }
}

impl std::fmt::Display for OutputValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSections(s) => write!(f, "Missing sections: {}", s.join(", ")),
            Self::InvalidJson(e) => write!(f, "Invalid JSON: {}", e),
        }
    }
}

/// Validate skill output against output config constraints.
///
/// Returns `Ok(output)` (possibly cleaned up, e.g. extracted JSON) or an error
/// with a repair prompt the caller can use for self-repair.
pub fn validate_skill_output(
    output: &str,
    output_config: &OutputConfig,
) -> Result<String, OutputValidationError> {
    if let Some(ref format) = output_config.format {
        match format.as_str() {
            "markdown" => {
                if !output_config.required_sections.is_empty() {
                    let missing =
                        find_missing_sections(output, &output_config.required_sections);
                    if !missing.is_empty() {
                        return Err(OutputValidationError::MissingSections(missing));
                    }
                }
            }
            "json" => {
                if serde_json::from_str::<serde_json::Value>(output).is_err() {
                    // Try to extract JSON from markdown code blocks
                    if let Some(json_str) = extract_json_from_output(output) {
                        serde_json::from_str::<serde_json::Value>(&json_str)
                            .map_err(|e| OutputValidationError::InvalidJson(e.to_string()))?;
                        return Ok(json_str);
                    }
                    return Err(OutputValidationError::InvalidJson(
                        "Output is not valid JSON".to_string(),
                    ));
                }
            }
            _ => {} // "text", "patchset", or unknown — no validation
        }
    }

    if let Some(max) = output_config.max_tokens {
        let estimated = output.len() / 4;
        if estimated > max {
            tracing::warn!(
                "Skill output exceeds max_tokens estimate ({} > {})",
                estimated,
                max
            );
        }
    }

    Ok(output.to_string())
}

fn find_missing_sections(output: &str, required: &[String]) -> Vec<String> {
    required
        .iter()
        .filter(|section| {
            let heading = format!("## {}", section);
            !output.contains(&heading)
        })
        .cloned()
        .collect()
}

fn extract_json_from_output(output: &str) -> Option<String> {
    // Look for ```json ... ``` blocks
    if let Some(start) = output.find("```json") {
        let content_start = start + "```json".len();
        if let Some(end) = output[content_start..].find("```") {
            return Some(output[content_start..content_start + end].trim().to_string());
        }
    }
    // Look for { ... }
    if let Some(start) = output.find('{')
        && let Some(end) = output.rfind('}')
        && end > start
    {
        return Some(output[start..=end].to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn config_markdown(sections: Vec<&str>) -> OutputConfig {
        OutputConfig {
            format: Some("markdown".to_string()),
            max_length: None,
            required_sections: sections.into_iter().map(|s| s.to_string()).collect(),
            max_tokens: None,
        }
    }

    fn config_json() -> OutputConfig {
        OutputConfig {
            format: Some("json".to_string()),
            max_length: None,
            required_sections: Vec::new(),
            max_tokens: None,
        }
    }

    #[test]
    fn test_no_format_passes() {
        let config = OutputConfig::default();
        let result = validate_skill_output("anything", &config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_markdown_all_sections_present() {
        let output = "## Summary\nAll good.\n\n## Details\nMore info.";
        let config = config_markdown(vec!["Summary", "Details"]);
        let result = validate_skill_output(output, &config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_markdown_missing_sections() {
        let output = "## Summary\nAll good.";
        let config = config_markdown(vec!["Summary", "Details", "Recommendations"]);
        let result = validate_skill_output(output, &config);
        assert!(result.is_err());
        if let Err(OutputValidationError::MissingSections(missing)) = result {
            assert_eq!(missing, vec!["Details", "Recommendations"]);
        } else {
            panic!("Expected MissingSections error");
        }
    }

    #[test]
    fn test_json_valid() {
        let output = r#"{"key": "value", "num": 42}"#;
        let config = config_json();
        let result = validate_skill_output(output, &config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_json_invalid() {
        let output = "This is not JSON at all";
        let config = config_json();
        let result = validate_skill_output(output, &config);
        assert!(result.is_err());
        assert!(matches!(result, Err(OutputValidationError::InvalidJson(_))));
    }

    #[test]
    fn test_json_extracted_from_code_block() {
        let output = "Here is the result:\n```json\n{\"status\": \"ok\"}\n```\nDone.";
        let config = config_json();
        let result = validate_skill_output(output, &config).unwrap();
        assert_eq!(result, r#"{"status": "ok"}"#);
    }

    #[test]
    fn test_json_extracted_from_braces() {
        let output = "Some text before {\"a\": 1} and after";
        let config = config_json();
        let result = validate_skill_output(output, &config).unwrap();
        assert_eq!(result, r#"{"a": 1}"#);
    }

    #[test]
    fn test_repair_prompt_missing_sections() {
        let err = OutputValidationError::MissingSections(vec![
            "Summary".to_string(),
            "Details".to_string(),
        ]);
        let prompt = err.repair_prompt();
        assert!(prompt.contains("Summary, Details"));
        assert!(prompt.contains("## SectionName"));
    }

    #[test]
    fn test_repair_prompt_invalid_json() {
        let err = OutputValidationError::InvalidJson("unexpected token".to_string());
        let prompt = err.repair_prompt();
        assert!(prompt.contains("unexpected token"));
        assert!(prompt.contains("valid JSON"));
    }

    #[test]
    fn test_max_tokens_warning_does_not_reject() {
        let output = "x".repeat(1000);
        let config = OutputConfig {
            format: Some("text".to_string()),
            max_tokens: Some(10), // 10 tokens ~= 40 chars, output is 1000
            ..Default::default()
        };
        // Should pass (warning only, not rejection)
        let result = validate_skill_output(&output, &config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_unknown_format_passes() {
        let config = OutputConfig {
            format: Some("patchset".to_string()),
            ..Default::default()
        };
        let result = validate_skill_output("anything", &config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_markdown_no_required_sections_passes() {
        let config = OutputConfig {
            format: Some("markdown".to_string()),
            required_sections: Vec::new(),
            ..Default::default()
        };
        let result = validate_skill_output("# Hello\nAnything goes", &config);
        assert!(result.is_ok());
    }
}
