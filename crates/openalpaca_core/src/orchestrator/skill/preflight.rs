//! Preflight permission checks for skill invocations.

use crate::middleware::skill::SkillFrontmatter;

/// Preflight check: validate that the skill's declared permissions are
/// consistent with its sandbox config.
pub(crate) fn preflight_permissions(frontmatter: &SkillFrontmatter) -> Result<(), String> {
    let level = &frontmatter.permissions.level;
    match level.as_str() {
        "readonly" => Ok(()),
        "readwrite" | "admin" => {
            // If the skill needs shell but sandbox disallows it, reject early
            if !frontmatter.permissions.sandbox.net
                && frontmatter.tools.allow.iter().any(|t| t == "web_fetch")
            {
                return Err("Skill requires web_fetch tool but sandbox.net is false".into());
            }
            Ok(())
        }
        other => Err(format!(
            "Unknown permission level '{}'. Valid levels: readonly, readwrite, admin",
            other
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::skill::{
        ConfirmAction, PermissionsConfig, SandboxConfig, SkillFrontmatter, ToolsConfig,
    };

    fn frontmatter_with_permission(level: &str) -> SkillFrontmatter {
        SkillFrontmatter {
            name: "test".to_string(),
            description: "test skill".to_string(),
            permissions: PermissionsConfig {
                level: level.to_string(),
                confirm: ConfirmAction::default(),
                sandbox: SandboxConfig::default(),
            },
            tools: ToolsConfig::default(),
            ..Default::default()
        }
    }

    #[test]
    fn test_preflight_readonly_passes() {
        let fm = frontmatter_with_permission("readonly");
        assert!(preflight_permissions(&fm).is_ok());
    }

    #[test]
    fn test_preflight_readwrite_passes() {
        let fm = frontmatter_with_permission("readwrite");
        assert!(preflight_permissions(&fm).is_ok());
    }

    #[test]
    fn test_preflight_admin_passes() {
        let fm = frontmatter_with_permission("admin");
        assert!(preflight_permissions(&fm).is_ok());
    }

    #[test]
    fn test_preflight_unknown_level_rejected() {
        let fm = frontmatter_with_permission("superuser");
        let result = preflight_permissions(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown permission level"));
    }

    #[test]
    fn test_preflight_web_fetch_without_net_rejected() {
        let fm = SkillFrontmatter {
            name: "test".to_string(),
            description: "test skill".to_string(),
            permissions: PermissionsConfig {
                level: "readwrite".to_string(),
                confirm: ConfirmAction::default(),
                sandbox: SandboxConfig {
                    enabled: true,
                    net: false,
                    fs_writable: Vec::new(),
                },
            },
            tools: ToolsConfig {
                allow: vec!["web_fetch".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        let result = preflight_permissions(&fm);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("web_fetch"));
    }

    #[test]
    fn test_confirm_tools_not_empty() {
        // Verify the ConfirmAction struct properly holds tools
        let confirm = ConfirmAction {
            tools: vec!["shell_execute".to_string(), "file_write".to_string()],
            message: Some("Are you sure?".to_string()),
        };
        assert_eq!(confirm.tools.len(), 2);
        assert_eq!(confirm.tools[0], "shell_execute");
    }
}
