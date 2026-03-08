use std::collections::HashSet;
use std::path::{Path, PathBuf};

use openalpaca_core::CoreError;
use serde_yaml::Value;

const INCLUDE_KEY: &str = "$include";
const MAX_INCLUDE_DEPTH: usize = 10;
const MAX_INCLUDE_FILE_BYTES: u64 = 2_000_000;

/// Resolve all `$include` directives in a parsed YAML value tree.
///
/// Include paths are resolved relative to the directory containing `config_path`.
pub fn resolve_includes(value: Value, config_path: &Path) -> Result<Value, CoreError> {
    let base_dir = config_path
        .parent()
        .ok_or_else(|| CoreError::InvalidConfig("config path has no parent directory".into()))?;
    let canonical_base = base_dir.canonicalize().map_err(|e| {
        CoreError::InvalidConfig(format!(
            "cannot canonicalize base dir {}: {e}",
            base_dir.display()
        ))
    })?;

    let mut visited = HashSet::new();
    if let Ok(canonical_self) = config_path.canonicalize() {
        visited.insert(canonical_self);
    }

    resolve_recursive(value, &canonical_base, &mut visited, 0)
}

fn resolve_recursive(
    value: Value,
    base_dir: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
) -> Result<Value, CoreError> {
    match value {
        Value::Mapping(mut map) => {
            let include_key = Value::String(INCLUDE_KEY.into());
            if let Some(include_val) = map.remove(&include_key) {
                // Collect include paths
                let paths = match &include_val {
                    Value::String(s) => vec![s.clone()],
                    Value::Sequence(seq) => {
                        let mut ps = Vec::with_capacity(seq.len());
                        for item in seq {
                            match item.as_str() {
                                Some(s) => ps.push(s.to_string()),
                                None => {
                                    return Err(CoreError::InvalidConfig(
                                        "$include array items must be strings".into(),
                                    ));
                                }
                            }
                        }
                        ps
                    }
                    _ => {
                        return Err(CoreError::InvalidConfig(
                            "$include value must be a string or array of strings".into(),
                        ));
                    }
                };

                // Start with an empty mapping as base, merge includes in order
                let mut merged = Value::Mapping(serde_yaml::Mapping::new());

                for rel_path in &paths {
                    let resolved = base_dir.join(rel_path);
                    let canonical = resolved.canonicalize().map_err(|e| {
                        CoreError::InvalidConfig(format!(
                            "cannot resolve include path '{}': {e}",
                            resolved.display()
                        ))
                    })?;

                    // Security: reject path traversal outside base_dir
                    if !canonical.starts_with(base_dir) {
                        return Err(CoreError::InvalidConfig(format!(
                            "include path '{}' escapes config directory",
                            rel_path
                        )));
                    }

                    // Cycle detection
                    if visited.contains(&canonical) {
                        return Err(CoreError::InvalidConfig(format!(
                            "circular include detected: {}",
                            canonical.display()
                        )));
                    }

                    // Depth check
                    if depth >= MAX_INCLUDE_DEPTH {
                        return Err(CoreError::InvalidConfig(format!(
                            "include depth exceeds maximum of {MAX_INCLUDE_DEPTH}"
                        )));
                    }

                    // Size check
                    let meta = std::fs::metadata(&canonical).map_err(|e| {
                        CoreError::InvalidConfig(format!(
                            "cannot read include file '{}': {e}",
                            canonical.display()
                        ))
                    })?;
                    if meta.len() > MAX_INCLUDE_FILE_BYTES {
                        return Err(CoreError::InvalidConfig(format!(
                            "include file '{}' exceeds {MAX_INCLUDE_FILE_BYTES} byte limit",
                            canonical.display()
                        )));
                    }

                    let content = std::fs::read_to_string(&canonical).map_err(|e| {
                        CoreError::InvalidConfig(format!(
                            "cannot read include file '{}': {e}",
                            canonical.display()
                        ))
                    })?;
                    let included_value: Value = serde_yaml::from_str(&content).map_err(|e| {
                        CoreError::InvalidConfig(format!(
                            "YAML parse error in included file '{}': {e}",
                            canonical.display()
                        ))
                    })?;

                    visited.insert(canonical.clone());
                    let included_dir = canonical.parent().unwrap_or(base_dir);
                    let processed =
                        resolve_recursive(included_value, included_dir, visited, depth + 1)?;

                    merged = deep_merge(merged, processed);
                }

                // Remaining siblings override included content
                // Recurse into sibling values first
                let mut sibling_map = serde_yaml::Mapping::new();
                for (k, v) in map {
                    sibling_map.insert(k, resolve_recursive(v, base_dir, visited, depth)?);
                }

                if sibling_map.is_empty() {
                    Ok(merged)
                } else {
                    Ok(deep_merge(merged, Value::Mapping(sibling_map)))
                }
            } else {
                // No $include — just recurse into all values
                let mut new_map = serde_yaml::Mapping::new();
                for (k, v) in map {
                    new_map.insert(k, resolve_recursive(v, base_dir, visited, depth)?);
                }
                Ok(Value::Mapping(new_map))
            }
        }
        Value::Sequence(seq) => {
            let mut new_seq = Vec::with_capacity(seq.len());
            for item in seq {
                new_seq.push(resolve_recursive(item, base_dir, visited, depth)?);
            }
            Ok(Value::Sequence(new_seq))
        }
        other => Ok(other),
    }
}

fn deep_merge(base: Value, patch: Value) -> Value {
    match (base, patch) {
        (Value::Mapping(mut base_map), Value::Mapping(patch_map)) => {
            for (k, patch_v) in patch_map {
                let merged_v = if let Some(base_v) = base_map.remove(&k) {
                    deep_merge(base_v, patch_v)
                } else {
                    patch_v
                };
                base_map.insert(k, merged_v);
            }
            Value::Mapping(base_map)
        }
        (Value::Sequence(mut base_seq), Value::Sequence(patch_seq)) => {
            base_seq.extend(patch_seq);
            Value::Sequence(base_seq)
        }
        (_, patch) => patch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_yaml(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn test_simple_include() {
        let dir = TempDir::new().unwrap();
        write_yaml(dir.path(), "base.yaml", "name: base\nport: 3000");
        let main = write_yaml(
            dir.path(),
            "main.yaml",
            "$include: base.yaml\nhost: localhost",
        );

        let raw: Value = serde_yaml::from_str(&std::fs::read_to_string(&main).unwrap()).unwrap();
        let result = resolve_includes(raw, &main).unwrap();

        let map = result.as_mapping().unwrap();
        assert_eq!(
            map.get(Value::String("name".into()))
                .unwrap()
                .as_str()
                .unwrap(),
            "base"
        );
        assert_eq!(
            map.get(Value::String("host".into()))
                .unwrap()
                .as_str()
                .unwrap(),
            "localhost"
        );
        assert_eq!(
            map.get(Value::String("port".into()))
                .unwrap()
                .as_u64()
                .unwrap(),
            3000
        );
    }

    #[test]
    fn test_array_include() {
        let dir = TempDir::new().unwrap();
        write_yaml(dir.path(), "a.yaml", "x: 1\nitems:\n  - a");
        write_yaml(dir.path(), "b.yaml", "y: 2\nitems:\n  - b");
        let main = write_yaml(dir.path(), "main.yaml", "$include:\n  - a.yaml\n  - b.yaml");

        let raw: Value = serde_yaml::from_str(&std::fs::read_to_string(&main).unwrap()).unwrap();
        let result = resolve_includes(raw, &main).unwrap();

        let map = result.as_mapping().unwrap();
        assert_eq!(
            map.get(Value::String("x".into()))
                .unwrap()
                .as_u64()
                .unwrap(),
            1
        );
        assert_eq!(
            map.get(Value::String("y".into()))
                .unwrap()
                .as_u64()
                .unwrap(),
            2
        );
        // Sequences should be concatenated: [a, b]
        let items = map
            .get(Value::String("items".into()))
            .unwrap()
            .as_sequence()
            .unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_str().unwrap(), "a");
        assert_eq!(items[1].as_str().unwrap(), "b");
    }

    #[test]
    fn test_include_with_siblings() {
        let dir = TempDir::new().unwrap();
        write_yaml(dir.path(), "base.yaml", "port: 3000\nhost: default");
        let main = write_yaml(
            dir.path(),
            "main.yaml",
            "$include: base.yaml\nhost: override",
        );

        let raw: Value = serde_yaml::from_str(&std::fs::read_to_string(&main).unwrap()).unwrap();
        let result = resolve_includes(raw, &main).unwrap();

        let map = result.as_mapping().unwrap();
        // Sibling should override included value
        assert_eq!(
            map.get(Value::String("host".into()))
                .unwrap()
                .as_str()
                .unwrap(),
            "override"
        );
        // Included value should still be present
        assert_eq!(
            map.get(Value::String("port".into()))
                .unwrap()
                .as_u64()
                .unwrap(),
            3000
        );
    }

    #[test]
    fn test_nested_include() {
        let dir = TempDir::new().unwrap();
        write_yaml(dir.path(), "leaf.yaml", "leaf: true");
        write_yaml(dir.path(), "mid.yaml", "$include: leaf.yaml\nmid: true");
        let main = write_yaml(dir.path(), "main.yaml", "$include: mid.yaml\ntop: true");

        let raw: Value = serde_yaml::from_str(&std::fs::read_to_string(&main).unwrap()).unwrap();
        let result = resolve_includes(raw, &main).unwrap();

        let map = result.as_mapping().unwrap();
        assert_eq!(
            map.get(Value::String("leaf".into()))
                .unwrap()
                .as_bool()
                .unwrap(),
            true
        );
        assert_eq!(
            map.get(Value::String("mid".into()))
                .unwrap()
                .as_bool()
                .unwrap(),
            true
        );
        assert_eq!(
            map.get(Value::String("top".into()))
                .unwrap()
                .as_bool()
                .unwrap(),
            true
        );
    }

    #[test]
    fn test_circular_include_detected() {
        let dir = TempDir::new().unwrap();
        write_yaml(dir.path(), "a.yaml", "$include: b.yaml");
        write_yaml(dir.path(), "b.yaml", "$include: a.yaml");
        let main = write_yaml(dir.path(), "main.yaml", "$include: a.yaml");

        let raw: Value = serde_yaml::from_str(&std::fs::read_to_string(&main).unwrap()).unwrap();
        let result = resolve_includes(raw, &main);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("circular include"), "got: {err}");
    }

    #[test]
    fn test_depth_limit() {
        let dir = TempDir::new().unwrap();
        // Create a chain of 12 files, each including the next
        for i in 0..12 {
            let next = format!("f{}.yaml", i + 1);
            let content = if i < 11 {
                format!("$include: {next}\nkey{i}: val{i}")
            } else {
                format!("key{i}: val{i}")
            };
            write_yaml(dir.path(), &format!("f{i}.yaml"), &content);
        }
        let main = write_yaml(dir.path(), "main.yaml", "$include: f0.yaml");

        let raw: Value = serde_yaml::from_str(&std::fs::read_to_string(&main).unwrap()).unwrap();
        let result = resolve_includes(raw, &main);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("depth exceeds"), "got: {err}");
    }

    #[test]
    fn test_path_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        let main = write_yaml(dir.path(), "main.yaml", "$include: \"../../etc/passwd\"");

        let raw: Value = serde_yaml::from_str(&std::fs::read_to_string(&main).unwrap()).unwrap();
        let result = resolve_includes(raw, &main);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("escapes config directory") || err.contains("cannot resolve"),
            "got: {err}"
        );
    }

    #[test]
    fn test_missing_include_file() {
        let dir = TempDir::new().unwrap();
        let main = write_yaml(dir.path(), "main.yaml", "$include: nonexistent.yaml");

        let raw: Value = serde_yaml::from_str(&std::fs::read_to_string(&main).unwrap()).unwrap();
        let result = resolve_includes(raw, &main);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("cannot resolve include path") || err.contains("nonexistent"),
            "got: {err}"
        );
    }
}
