//! Annotation-based virtual capabilities for tool resolution.
//!
//! Lets agents declare needs like `annotation:readonly` and have the registry
//! resolve to tools with `read_only_hint = Some(true)`. Extension point:
//! `CapabilityProvider` trait allows downstream code to declare additional
//! virtual capabilities.

use super::RegisteredTool;

/// The 8 valid suffixes for the `annotation:` capability prefix
/// (4 positive + 4 inverse).
pub const ANNOTATION_CAPABILITY_NAMES: &[&str] = &[
    "annotation:readonly",
    "annotation:destructive",
    "annotation:idempotent",
    "annotation:open_world",
    "annotation:non_readonly",
    "annotation:non_destructive",
    "annotation:non_idempotent",
    "annotation:non_open_world",
];

/// Derive the set of annotation capability strings a tool provides based on
/// its MCP annotations. Only `Some(true)` hints produce positive capabilities;
/// only `Some(false)` hints produce inverse (`non_*`) capabilities. `None`
/// hints produce nothing.
pub fn annotation_capabilities(
    annotations: Option<&openalpaca_mcp::ToolAnnotations>,
) -> Vec<String> {
    let Some(a) = annotations else { return Vec::new(); };
    let mut caps = Vec::new();
    match a.read_only_hint {
        Some(true) => caps.push("annotation:readonly".into()),
        Some(false) => caps.push("annotation:non_readonly".into()),
        None => {}
    }
    match a.destructive_hint {
        Some(true) => caps.push("annotation:destructive".into()),
        Some(false) => caps.push("annotation:non_destructive".into()),
        None => {}
    }
    match a.idempotent_hint {
        Some(true) => caps.push("annotation:idempotent".into()),
        Some(false) => caps.push("annotation:non_idempotent".into()),
        None => {}
    }
    match a.open_world_hint {
        Some(true) => caps.push("annotation:open_world".into()),
        Some(false) => caps.push("annotation:non_open_world".into()),
        None => {}
    }
    caps
}

/// Validate that a capability string with the `annotation:` prefix is one of
/// the known names. Non-prefixed strings are unaffected and always pass.
pub fn validate_annotation_capability(
    cap: &str,
    known: &[&'static str],
) -> Result<(), String> {
    if !cap.starts_with("annotation:") {
        return Ok(());
    }
    if known.contains(&cap) {
        return Ok(());
    }
    Err(format!(
        "unknown annotation capability '{cap}' — valid names: {}",
        known.join(", ")
    ))
}

/// Derives virtual capability strings from a `RegisteredTool`.
///
/// Built-in providers cover MCP annotation hints; plugins or downstream
/// code can register additional providers for domain-specific capabilities.
///
/// Providers must be pure — deterministic and side-effect-free.
pub trait CapabilityProvider: Send + Sync {
    /// Return the virtual capabilities this tool has according to this provider.
    fn derive_capabilities(&self, tool: &RegisteredTool) -> Vec<String>;

    /// Return the set of capability names this provider can produce, used
    /// for agent-config validation. Must be a compile-time-fixed `&'static`.
    fn known_capability_names(&self) -> &'static [&'static str];
}

/// Built-in provider for the 8 MCP-derived annotation capabilities.
pub struct AnnotationCapabilityProvider;

impl CapabilityProvider for AnnotationCapabilityProvider {
    fn derive_capabilities(&self, tool: &RegisteredTool) -> Vec<String> {
        annotation_capabilities(tool.annotations.as_ref())
    }

    fn known_capability_names(&self) -> &'static [&'static str] {
        ANNOTATION_CAPABILITY_NAMES
    }
}

/// Opaque identifier returned from `register_capability_provider`.
///
/// Used to reference a provider for removal via `remove_capability_provider`.
/// Handles are process-unique and monotonically increasing. They do NOT
/// survive daemon restarts — re-registering after restart returns a fresh handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProviderHandle(pub(crate) u64);

impl std::fmt::Display for ProviderHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProviderHandle({})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ann(
        destructive: Option<bool>,
        readonly: Option<bool>,
        idempotent: Option<bool>,
        open_world: Option<bool>,
    ) -> openalpaca_mcp::ToolAnnotations {
        openalpaca_mcp::ToolAnnotations {
            destructive_hint: destructive,
            read_only_hint: readonly,
            idempotent_hint: idempotent,
            open_world_hint: open_world,
            ..Default::default()
        }
    }

    #[test]
    fn annotation_capabilities_none_returns_empty() {
        assert!(annotation_capabilities(None).is_empty());
    }

    #[test]
    fn annotation_capabilities_all_hints_some_true() {
        let a = ann(Some(true), Some(true), Some(true), Some(true));
        let caps = annotation_capabilities(Some(&a));
        assert_eq!(caps.len(), 4);
        assert!(caps.contains(&"annotation:readonly".to_string()));
        assert!(caps.contains(&"annotation:destructive".to_string()));
        assert!(caps.contains(&"annotation:idempotent".to_string()));
        assert!(caps.contains(&"annotation:open_world".to_string()));
    }

    #[test]
    fn annotation_capabilities_all_hints_some_false() {
        let a = ann(Some(false), Some(false), Some(false), Some(false));
        let caps = annotation_capabilities(Some(&a));
        assert_eq!(caps.len(), 4);
        assert!(caps.contains(&"annotation:non_readonly".to_string()));
        assert!(caps.contains(&"annotation:non_destructive".to_string()));
        assert!(caps.contains(&"annotation:non_idempotent".to_string()));
        assert!(caps.contains(&"annotation:non_open_world".to_string()));
    }

    #[test]
    fn annotation_capabilities_mixed_hints() {
        let a = ann(Some(true), Some(false), None, Some(true));
        let caps = annotation_capabilities(Some(&a));
        assert_eq!(caps.len(), 3);
        assert!(caps.contains(&"annotation:destructive".to_string()));
        assert!(caps.contains(&"annotation:non_readonly".to_string()));
        assert!(caps.contains(&"annotation:open_world".to_string()));
        assert!(!caps.iter().any(|c| c.contains("idempotent")));
    }

    #[test]
    fn annotation_capabilities_ignores_none_hints() {
        let a = ann(None, None, None, None);
        let caps = annotation_capabilities(Some(&a));
        assert!(caps.is_empty());
    }

    #[test]
    fn validate_passes_non_annotation_strings() {
        let known = ANNOTATION_CAPABILITY_NAMES;
        assert!(validate_annotation_capability("file_read", known).is_ok());
        assert!(validate_annotation_capability("shell_execute", known).is_ok());
        assert!(validate_annotation_capability("messaging", known).is_ok());
    }

    #[test]
    fn validate_passes_known_annotation_names() {
        let known = ANNOTATION_CAPABILITY_NAMES;
        for name in ANNOTATION_CAPABILITY_NAMES {
            assert!(
                validate_annotation_capability(name, known).is_ok(),
                "{name} should validate"
            );
        }
    }

    #[test]
    fn validate_rejects_typo_in_annotation_name() {
        let known = ANNOTATION_CAPABILITY_NAMES;
        let err = validate_annotation_capability("annotation:redaonly", known).unwrap_err();
        assert!(err.contains("unknown annotation capability"));
        assert!(err.contains("annotation:redaonly"));
        assert!(err.contains("annotation:readonly"));
    }

    #[test]
    fn validate_rejects_bare_prefix() {
        let known = ANNOTATION_CAPABILITY_NAMES;
        let err = validate_annotation_capability("annotation:", known).unwrap_err();
        assert!(err.contains("unknown annotation capability"));
    }

    #[test]
    fn validate_error_lists_all_known_names() {
        let known = ANNOTATION_CAPABILITY_NAMES;
        let err = validate_annotation_capability("annotation:foo", known).unwrap_err();
        for name in ANNOTATION_CAPABILITY_NAMES {
            assert!(err.contains(name), "error should mention {name}");
        }
    }

    #[test]
    fn provider_handle_display_format() {
        assert_eq!(ProviderHandle(42).to_string(), "ProviderHandle(42)");
        assert_eq!(ProviderHandle(0).to_string(), "ProviderHandle(0)");
    }

    #[test]
    fn provider_handles_are_comparable() {
        use std::collections::HashSet;
        let a = ProviderHandle(1);
        let b = ProviderHandle(2);
        let c = ProviderHandle(1);
        assert_eq!(a, c);
        assert_ne!(a, b);
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&c));
        assert!(!set.contains(&b));
    }

    #[test]
    fn provider_handle_can_be_copied() {
        let h = ProviderHandle(42);
        let h2 = h;  // Copy
        assert_eq!(h, h2);
    }
}
