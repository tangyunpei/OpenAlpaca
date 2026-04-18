//! Annotations block in user TOML tool configs.

use serde::Deserialize;

/// Annotations declaration in user TOML, mapping 1:1 to MCP hint fields.
/// All fields optional; `None` means "no opinion" (same as MCP semantics).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolAnnotationsConfig {
    #[serde(default)]
    pub destructive_hint: Option<bool>,
    #[serde(default)]
    pub read_only_hint: Option<bool>,
    #[serde(default)]
    pub idempotent_hint: Option<bool>,
    #[serde(default)]
    pub open_world_hint: Option<bool>,
}

impl ToolAnnotationsConfig {
    pub fn into_mcp_annotations(self) -> openalpaca_mcp::ToolAnnotations {
        openalpaca_mcp::ToolAnnotations {
            destructive_hint: self.destructive_hint,
            read_only_hint: self.read_only_hint,
            idempotent_hint: self.idempotent_hint,
            open_world_hint: self.open_world_hint,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_converts_to_default_annotations() {
        let cfg = ToolAnnotationsConfig::default();
        let ann = cfg.into_mcp_annotations();
        assert_eq!(ann.destructive_hint, None);
        assert_eq!(ann.read_only_hint, None);
        assert_eq!(ann.idempotent_hint, None);
        assert_eq!(ann.open_world_hint, None);
    }

    #[test]
    fn full_config_roundtrips() {
        let cfg = ToolAnnotationsConfig {
            destructive_hint: Some(true),
            read_only_hint: Some(false),
            idempotent_hint: Some(false),
            open_world_hint: Some(true),
        };
        let ann = cfg.into_mcp_annotations();
        assert_eq!(ann.destructive_hint, Some(true));
        assert_eq!(ann.read_only_hint, Some(false));
        assert_eq!(ann.idempotent_hint, Some(false));
        assert_eq!(ann.open_world_hint, Some(true));
    }

    #[test]
    fn partial_config_leaves_others_none() {
        let toml_text = "destructive_hint = true";
        let cfg: ToolAnnotationsConfig = toml::from_str(toml_text).unwrap();
        let ann = cfg.into_mcp_annotations();
        assert_eq!(ann.destructive_hint, Some(true));
        assert_eq!(ann.read_only_hint, None);
        assert_eq!(ann.idempotent_hint, None);
        assert_eq!(ann.open_world_hint, None);
    }
}
